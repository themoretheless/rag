//! Unified Library workspace: lean server-side catalog with deliberate filters.

use crate::load::DocumentBody;
use crate::product::{LibraryItem, LibraryPage, LibraryRequest};
use crate::ui::document::draw_document_reader;
use crate::ui::{closing_selectable_value, theme};

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
    let filters_dirty = page.is_some_and(|page| !library_filters_match(request, &page.request));
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.heading("Единый корпус");
            ui.weak("Все индексированные документы; содержимое загружается только при открытии.");
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            let refresh_label = if filters_dirty {
                "Применить и обновить"
            } else {
                "Обновить"
            };
            if ui
                .add_enabled(!loading, egui::Button::new(refresh_label))
                .clicked()
            {
                action = if filters_dirty {
                    LibraryAction::ApplyFilters
                } else {
                    LibraryAction::Refresh
                };
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
                        .hint_text("Название, URI или исходный файл…"),
                );
                if search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    action = LibraryAction::ApplyFilters;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut request.room)
                        .hint_text("Комната")
                        .desired_width(100.0),
                );
                filter_combo(ui, "library_layer", "Слой", &mut request.layer, LAYERS);
                filter_combo(ui, "library_kind", "Тип", &mut request.kind, KINDS);
                filter_combo(
                    ui,
                    "library_status",
                    "Статус",
                    &mut request.status,
                    STATUSES,
                );
                ui.checkbox(&mut request.include_archived, "Включая архив");
                if ui.button("Применить").clicked() {
                    action = LibraryAction::ApplyFilters;
                }
                if ui.button("Сбросить").clicked() {
                    action = LibraryAction::ResetFilters;
                }
            });
        });

    ui.add_space(10.0);
    if let Some(error) = error {
        ui.colored_label(
            egui::Color32::from_rgb(215, 100, 85),
            format!("Не удалось загрузить корпус: {error}"),
        );
        ui.add_space(8.0);
    }

    let Some(page) = page else {
        if loading {
            ui.add_space(50.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.weak("Загружаю каталог…");
            });
        } else if error.is_none() {
            ui.weak("Каталог ещё не загружен.");
        }
        return action;
    };

    let filters_dirty = !library_filters_match(request, &page.request);
    if filters_dirty {
        ui.colored_label(
            egui::Color32::from_rgb(220, 150, 65),
            "Фильтры изменены. Примените их перед переключением страниц.",
        );
        ui.add_space(5.0);
    }

    ui.horizontal(|ui| {
        ui.strong(format!("Документов: {}", page.total));
        let scope = page.request.wing.trim();
        if !scope.is_empty() {
            ui.separator();
            ui.weak(format!("Проект: {scope}"));
        }
        let active_filters = active_filter_count(&page.request);
        if active_filters > 0 {
            ui.separator();
            ui.weak(format!("Активных фильтров: {active_filters}"));
        }
    });
    ui.add_space(5.0);

    if page.items.is_empty() {
        egui::Frame::group(ui.style())
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.strong("Документы не найдены");
                ui.label("Сбросьте фильтры или выберите другой проект.");
            });
    } else {
        const TITLE_MIN_WIDTH: f32 = 240.0;
        const SCOPE_WIDTH: f32 = 190.0;
        const LAYER_WIDTH: f32 = 88.0;
        const KIND_WIDTH: f32 = 110.0;
        const UPDATED_WIDTH: f32 = 176.0;
        const COLUMN_SPACING: f32 = 14.0;
        const COLUMN_GAPS: f32 = COLUMN_SPACING * 4.0;

        let scroll_reserve =
            ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_outer_margin * 2.0;
        let fixed_width = SCOPE_WIDTH + LAYER_WIDTH + KIND_WIDTH + UPDATED_WIDTH + COLUMN_GAPS;
        let title_width =
            (ui.available_width() - fixed_width - scroll_reserve).max(TITLE_MIN_WIDTH);
        let table_width = title_width + fixed_width;
        let row_height = ui.spacing().interact_size.y;

        egui::ScrollArea::both()
            .min_scrolled_width(table_width)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(table_width);
                egui::Grid::new("unified_library_rows")
                    .num_columns(5)
                    .striped(true)
                    .min_col_width(0.0)
                    .spacing([COLUMN_SPACING, 9.0])
                    .show(ui, |ui| {
                        table_header(ui, "Документ", title_width, row_height);
                        table_header(ui, "Контекст", SCOPE_WIDTH, row_height);
                        table_header(ui, "Слой", LAYER_WIDTH, row_height);
                        table_header(ui, "Тип", KIND_WIDTH, row_height);
                        table_header(ui, "Обновлён", UPDATED_WIDTH, row_height);
                        ui.end_row();
                        for item in &page.items {
                            let selected = selected_id == Some(item.id.as_str());
                            let mut title = item.title.trim();
                            if title.is_empty() {
                                title = item.uri.as_str();
                            }
                            let title = if item.pinned {
                                format!("PIN · {title}")
                            } else {
                                title.to_string()
                            };
                            let title_response = ui
                                .add_sized(
                                    [title_width, row_height],
                                    egui::Button::selectable(selected, title.as_str()).truncate(),
                                )
                                .on_hover_text(&title);
                            if title_response.clicked() {
                                action = LibraryAction::Select(item.id.clone());
                            }
                            let scope = item.scope_label();
                            ui.add_sized(
                                [SCOPE_WIDTH, row_height],
                                egui::Label::new(
                                    egui::RichText::new(scope.as_str()).color(theme::MUTED),
                                )
                                .truncate(),
                            )
                            .on_hover_text(scope);
                            ui.allocate_ui_with_layout(
                                egui::vec2(LAYER_WIDTH, row_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| badge(ui, &item.layer, layer_color(&item.layer)),
                            );
                            ui.add_sized(
                                [KIND_WIDTH, row_height],
                                egui::Label::new(kind_label(&item.kind)).truncate(),
                            );
                            ui.add_sized(
                                [UPDATED_WIDTH, row_height],
                                egui::Label::new(
                                    egui::RichText::new(short_timestamp(&item.updated_at))
                                        .color(theme::MUTED),
                                )
                                .truncate(),
                            );
                            ui.end_row();
                        }
                    });
            });
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let previous = ui.add_enabled(
            has_previous_page && !loading && !filters_dirty,
            egui::Button::new("< Назад"),
        );
        let previous = if filters_dirty {
            previous
                .on_disabled_hover_text("Примените изменённые фильтры перед переключением страниц")
        } else {
            previous
        };
        if previous.clicked() {
            action = LibraryAction::PreviousPage;
        }
        let has_next = page.next_cursor.is_some();
        let next = ui.add_enabled(
            has_next && !loading && !filters_dirty,
            egui::Button::new("Вперёд >"),
        );
        let next = if filters_dirty {
            next.on_disabled_hover_text("Примените изменённые фильтры перед переключением страниц")
        } else {
            next
        };
        if next.clicked() {
            if let Some(cursor) = &page.next_cursor {
                action = LibraryAction::NextPage(cursor.clone());
            }
        }
        ui.separator();
        ui.weak(format!("На странице: {}", page.items.len()));
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
        ui.label(
            egui::RichText::new("ПРЕДПРОСМОТР")
                .monospace()
                .small()
                .color(theme::FAINT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Закрыть").clicked() {
                action = LibraryDetailAction::Close;
            }
        });
    });
    ui.add_space(10.0);
    ui.heading(if item.title.trim().is_empty() {
        &item.uri
    } else {
        &item.title
    });
    ui.horizontal_wrapped(|ui| {
        badge(ui, &item.layer, layer_color(&item.layer));
        badge(
            ui,
            kind_label(&item.kind),
            egui::Color32::from_rgb(100, 120, 155),
        );
        if item.status != "active" {
            badge(
                ui,
                status_label(&item.status),
                egui::Color32::from_rgb(190, 125, 65),
            );
        }
        if item.pinned {
            ui.label(egui::RichText::new("PIN · закреплён").color(theme::WARN));
        }
    });
    ui.add_space(9.0);
    theme::inset().show(ui, |ui| {
        metadata_row(ui, "Проект / комната", &item.scope_label());
        metadata_row(ui, "URI", &item.uri);
        if let Some(source_file) = item.source_file.as_deref() {
            metadata_row(ui, "Источник", source_file);
        }
        metadata_row(ui, "Обновлён", &short_timestamp(&item.updated_at));
    });
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if body.is_some() && ui.button("История").clicked() {
            action = LibraryDetailAction::History(item.id.clone());
        }
        if item.is_wiki() && ui.button("Открыть в Вики").clicked() {
            action = LibraryDetailAction::OpenWiki(item.id.clone());
        }
        if ui.button("Показать связи").clicked() {
            action = LibraryDetailAction::OpenGraph(item.id.clone());
        }
    });
    ui.separator();

    if loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Загружаю полный документ…");
        });
    }
    if let Some(error) = error {
        ui.colored_label(egui::Color32::from_rgb(215, 100, 85), error);
        if ui.button("Повторить").clicked() {
            action = LibraryDetailAction::Retry;
        }
    }
    if let Some(body) = body {
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("library_document_reader")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                draw_document_reader(ui, &body.content, &body.kind, &body.uri);
            });
    }
    action
}

const LAYERS: &[(&str, &str)] = &[("raw", "Raw"), ("wiki", "Wiki"), ("diary", "Журнал")];
const KINDS: &[(&str, &str)] = &[
    ("document", "Документ"),
    ("wiki", "Wiki"),
    ("diary", "Журнал"),
    ("code", "Код"),
];
const STATUSES: &[(&str, &str)] = &[
    ("active", "Активен"),
    ("archived", "В архиве"),
    ("tombstone", "Метка удаления"),
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
            closing_selectable_value(ui, value, String::new(), empty_label);
            for (key, label) in choices {
                closing_selectable_value(ui, value, (*key).to_string(), *label);
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

fn table_header(ui: &mut egui::Ui, text: &str, width: f32, height: f32) {
    ui.add_sized(
        [width, height],
        egui::Label::new(egui::RichText::new(text).strong()).truncate(),
    );
}

fn metadata_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!("{label}:"));
        ui.label(value);
    });
}

fn layer_color(layer: &str) -> egui::Color32 {
    theme::layer_color(layer)
}

fn short_timestamp(value: &str) -> String {
    value
        .strip_suffix('Z')
        .unwrap_or(value)
        .replace('T', " ")
        .chars()
        .take(19)
        .collect()
}

fn kind_label(value: &str) -> &str {
    match value {
        "document" => "документ",
        "wiki" => "wiki",
        "diary" => "журнал",
        "code" => "код",
        _ => value,
    }
}

fn status_label(value: &str) -> &str {
    match value {
        "active" => "активен",
        "archived" => "в архиве",
        "tombstone" => "метка удаления",
        _ => value,
    }
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

fn library_filters_match(left: &LibraryRequest, right: &LibraryRequest) -> bool {
    let same_text = |left: &str, right: &str| left.trim() == right.trim();
    same_text(&left.q, &right.q)
        && same_text(&left.wing, &right.wing)
        && same_text(&left.room, &right.room)
        && same_text(&left.layer, &right.layer)
        && same_text(&left.kind, &right.kind)
        && same_text(&left.status, &right.status)
        && left.include_archived == right.include_archived
        && left.limit.clamp(1, 200) == right.limit.clamp(1, 200)
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
    fn cursor_is_reusable_only_while_applied_filters_match() {
        let applied = LibraryRequest {
            q: "design".to_string(),
            wing: "alpha".to_string(),
            cursor: Some("v1:50".to_string()),
            limit: 50,
            ..LibraryRequest::default()
        };
        let mut draft = applied.clone();
        draft.cursor = Some("v1:100".to_string());
        assert!(library_filters_match(&draft, &applied));

        draft.room = "docs".to_string();
        assert!(!library_filters_match(&draft, &applied));
    }

    #[test]
    fn timestamps_are_compact_for_table_rows() {
        assert_eq!(
            short_timestamp("2026-09-02T12:34:56Z"),
            "2026-09-02 12:34:56"
        );
        assert_eq!(
            short_timestamp("2026-09-02T12:34:56.123456Z"),
            "2026-09-02 12:34:56"
        );
    }

    #[test]
    fn unknown_layer_gets_neutral_color() {
        assert_eq!(layer_color("custom"), theme::MUTED);
    }
}
