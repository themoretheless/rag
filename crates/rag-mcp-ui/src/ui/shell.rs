//! Native application shell shared by every workspace.

use egui::{
    Align, Align2, Button, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};

use super::{closing_selectable_value, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellRoute {
    Console,
    Corpus,
    Search,
    Graph,
    Wiki,
    Agents,
    Evaluation,
    Models,
}

impl ShellRoute {
    pub const fn requires_http(self) -> bool {
        matches!(
            self,
            Self::Corpus | Self::Search | Self::Agents | Self::Evaluation | Self::Models
        )
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Console => "Пульт",
            Self::Corpus => "Корпус",
            Self::Search => "Поиск",
            Self::Graph => "Граф",
            Self::Wiki => "Вики",
            Self::Agents => "Операции · Журнал",
            Self::Evaluation => "Оценка",
            Self::Models => "Runtime и индекс",
        }
    }

    pub const fn layer(self) -> &'static str {
        match self {
            Self::Console => "L0–L4 · обзор",
            Self::Corpus => "L0 · сырой корпус",
            Self::Search => "L1 · извлечение",
            Self::Graph => "L2 · граф объектов",
            Self::Wiki => "L3 · знание",
            Self::Agents => "L4 · HTTP/MCP события",
            Self::Evaluation => "eval · retrieval",
            Self::Models => "gateway · status/doctor",
        }
    }

    pub const fn color(self) -> Color32 {
        match self {
            Self::Console => theme::ACCENT,
            Self::Corpus => theme::L0,
            Self::Search | Self::Evaluation => theme::L1,
            Self::Graph => theme::L2,
            Self::Wiki | Self::Models => theme::L3,
            Self::Agents => theme::L4,
        }
    }
}

#[derive(Debug, Default)]
pub struct RailOutput {
    pub navigate: Option<ShellRoute>,
}

pub fn draw_rail(
    root_ui: &mut egui::Ui,
    current: ShellRoute,
    http_available: bool,
    wiki_available: bool,
) -> RailOutput {
    let mut output = RailOutput::default();
    egui::Panel::left("console_primary_rail")
        .exact_size(56.0)
        .resizable(false)
        .frame(theme::rail_frame())
        .show(root_ui, |ui| {
            ui.set_width(48.0);
            let mark = ui.add_sized(
                [40.0, 36.0],
                Button::new(RichText::new("R").strong().size(16.0).color(theme::RAIL))
                    .fill(theme::L3)
                    .stroke(Stroke::new(1.0, theme::L0)),
            );
            if mark.clicked() {
                output.navigate = Some(ShellRoute::Console);
            }
            mark.on_hover_text("RAG Console");
            ui.add_space(8.0);

            for (route, label) in [
                (ShellRoute::Console, "Пульт"),
                (ShellRoute::Corpus, "Корпус"),
                (ShellRoute::Search, "Поиск"),
                (ShellRoute::Graph, "Граф"),
                (ShellRoute::Wiki, "Вики"),
                (ShellRoute::Agents, "Журнал"),
                (ShellRoute::Evaluation, "Оценка"),
            ] {
                let enabled = match route {
                    ShellRoute::Wiki => wiki_available,
                    _ => http_available || !route.requires_http(),
                };
                let response = ui.add_enabled_ui(enabled, |ui| {
                    rail_button(ui, route == current, route, label, route.color())
                });
                if response.inner.clicked() {
                    output.navigate = Some(route);
                }
                if !enabled {
                    let reason = if route == ShellRoute::Wiki {
                        "Вики недоступна в graph snapshot"
                    } else {
                        "Этот раздел требует HTTP gateway"
                    };
                    response.response.on_disabled_hover_text(reason);
                }
            }

            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                let response = ui.add_enabled_ui(http_available, |ui| {
                    rail_button(
                        ui,
                        current == ShellRoute::Models,
                        ShellRoute::Models,
                        "Runtime",
                        ShellRoute::Models.color(),
                    )
                });
                if response.inner.clicked() {
                    output.navigate = Some(ShellRoute::Models);
                }
                if !http_available {
                    response
                        .response
                        .on_disabled_hover_text("Runtime status требует HTTP gateway");
                }
            });
        });
    output
}

fn rail_button(
    ui: &mut egui::Ui,
    selected: bool,
    route: ShellRoute,
    label: &str,
    color: Color32,
) -> egui::Response {
    let text_color = if selected { theme::TEXT } else { theme::FAINT };
    let fill = if selected {
        theme::rgba(color, 28)
    } else {
        Color32::TRANSPARENT
    };
    let response = ui.add_sized(
        [48.0, 51.0],
        // Keep a real accessible label while all visible icon geometry is
        // painted. This avoids tofu squares when a system font misses symbols.
        Button::new(RichText::new(label).color(Color32::TRANSPARENT))
            .frame(false)
            .fill(fill)
            .sense(Sense::click()),
    );
    let icon_color = if selected { color } else { text_color };
    paint_route_icon(ui.painter(), route, response.rect, icon_color);
    ui.painter().text(
        Pos2::new(response.rect.center().x, response.rect.bottom() - 8.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(10.5),
        text_color,
    );
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                response.rect.left_top(),
                Vec2::new(2.0, response.rect.height()),
            ),
            1.0,
            color,
        );
    }
    response.on_hover_text(label)
}

fn paint_route_icon(painter: &egui::Painter, route: ShellRoute, button: Rect, color: Color32) {
    let center = Pos2::new(button.center().x, button.top() + 16.0);
    let stroke = Stroke::new(1.5, color);
    match route {
        ShellRoute::Console => {
            for offset in [(-5.0, -5.0), (1.0, -5.0), (-5.0, 1.0), (1.0, 1.0)] {
                painter.rect_stroke(
                    Rect::from_min_size(center + egui::vec2(offset.0, offset.1), Vec2::splat(4.0)),
                    0.8,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        ShellRoute::Corpus => {
            painter.line_segment(
                [
                    center + egui::vec2(-6.0, -5.0),
                    center + egui::vec2(6.0, -5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-6.0, 0.0),
                    center + egui::vec2(6.0, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-6.0, 5.0),
                    center + egui::vec2(6.0, 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-6.0, -5.0),
                    center + egui::vec2(-6.0, 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(6.0, -5.0),
                    center + egui::vec2(6.0, 5.0),
                ],
                stroke,
            );
        }
        ShellRoute::Search => paint_search_icon(painter, center, color),
        ShellRoute::Graph => {
            let left = center + egui::vec2(-6.0, 4.0);
            let top = center + egui::vec2(0.0, -5.0);
            let right = center + egui::vec2(6.0, 4.0);
            painter.line_segment([left, top], stroke);
            painter.line_segment([top, right], stroke);
            painter.line_segment([left, right], stroke);
            for point in [left, top, right] {
                painter.circle_filled(point, 2.2, color);
            }
        }
        ShellRoute::Wiki => {
            painter.rect_stroke(
                Rect::from_center_size(center, egui::vec2(13.0, 12.0)),
                1.5,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -6.0),
                    center + egui::vec2(0.0, 6.0),
                ],
                stroke,
            );
        }
        ShellRoute::Agents => {
            for y in [-4.0, 0.0, 4.0] {
                painter.circle_filled(center + egui::vec2(-5.0, y), 1.2, color);
                painter.line_segment(
                    [center + egui::vec2(-1.5, y), center + egui::vec2(6.0, y)],
                    stroke,
                );
            }
        }
        ShellRoute::Evaluation => {
            for (x, height) in [(-5.0, 5.0), (0.0, 9.0), (5.0, 12.0)] {
                painter.rect_filled(
                    Rect::from_min_max(
                        center + egui::vec2(x - 1.5, 6.0 - height),
                        center + egui::vec2(x + 1.5, 6.0),
                    ),
                    0.8,
                    color,
                );
            }
        }
        ShellRoute::Models => {
            painter.circle_stroke(center, 5.5, stroke);
            painter.circle_filled(center, 1.8, color);
            painter.line_segment(
                [
                    center + egui::vec2(-8.0, 0.0),
                    center + egui::vec2(-5.5, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(5.5, 0.0), center + egui::vec2(8.0, 0.0)],
                stroke,
            );
        }
    }
}

fn paint_search_icon(painter: &egui::Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let lens = center + egui::vec2(-1.5, -1.5);
    painter.circle_stroke(lens, 4.5, stroke);
    painter.line_segment(
        [lens + egui::vec2(3.2, 3.2), lens + egui::vec2(7.0, 7.0)],
        stroke,
    );
}

fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(9.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

pub struct TopbarState<'a> {
    pub route: ShellRoute,
    pub project: &'a mut String,
    pub projects: &'a [String],
    pub project_enabled: bool,
    pub project_loading: bool,
    pub project_error: Option<&'a str>,
    pub search_enabled: bool,
    pub connected: bool,
    pub read_only_source: Option<&'a str>,
    pub healthy: Option<bool>,
    pub health_summary: &'a str,
    pub mutation_label: Option<&'a str>,
}

#[derive(Debug, Default)]
pub struct TopbarOutput {
    pub retry_projects: bool,
    pub open_search: bool,
}

pub fn draw_topbar(root_ui: &mut egui::Ui, state: TopbarState<'_>) -> TopbarOutput {
    let mut output = TopbarOutput::default();
    egui::Panel::top("console_global_topbar")
        .exact_size(48.0)
        .resizable(false)
        .frame(theme::topbar_frame())
        .show(root_ui, |ui| {
            let compact = ui.available_width() < 1_180.0;
            let breadcrumb_width = if compact { 210.0 } else { 345.0 };
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(breadcrumb_width, 30.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        if !compact {
                            ui.label(RichText::new("rag-mcp").color(theme::FAINT));
                            ui.label(RichText::new("/").color(theme::FAINT));
                        }
                        status_dot(ui, state.route.color());
                        ui.strong(state.route.title());
                        if !compact {
                            let badge = RichText::new(state.route.layer())
                                .monospace()
                                .size(10.0)
                                .color(state.route.color());
                            egui::Frame::new()
                                .fill(theme::rgba(state.route.color(), 22))
                                .corner_radius(5.0)
                                .inner_margin(egui::Margin::symmetric(6, 3))
                                .show(ui, |ui| {
                                    ui.label(badge);
                                });
                        }
                    },
                );

                let search_width = if compact {
                    220.0
                } else {
                    (ui.available_width() * 0.38).clamp(280.0, 390.0)
                };
                let search = ui.add_enabled(
                    state.search_enabled,
                    Button::new(
                        RichText::new("    Поиск по знаниям…")
                            .size(12.0)
                            .color(theme::FAINT),
                    )
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .min_size(egui::vec2(search_width, 30.0)),
                );
                paint_search_icon(
                    ui.painter(),
                    search.rect.left_center() + egui::vec2(17.0, -1.0),
                    theme::FAINT,
                );
                if state.search_enabled {
                    ui.painter().text(
                        search.rect.right_center() - egui::vec2(9.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        "Cmd K",
                        egui::FontId::monospace(10.0),
                        theme::FAINT,
                    );
                }
                let search = if state.search_enabled {
                    search.on_hover_text("Открыть поиск")
                } else {
                    search.on_disabled_hover_text("Поиск требует HTTP gateway")
                };
                if search.clicked() {
                    output.open_search = true;
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let dot = match (state.healthy, state.read_only_source) {
                        (Some(true), _) => theme::OK,
                        (Some(false), _) => theme::DANGER,
                        (None, Some(_)) => theme::ACCENT,
                        (None, None) if state.connected => theme::WARN,
                        (None, None) => theme::DANGER,
                    };
                    ui.horizontal(|ui| {
                        status_dot(ui, dot);
                        if !compact {
                            ui.label(
                                RichText::new(state.health_summary)
                                    .monospace()
                                    .size(10.0)
                                    .color(theme::MUTED),
                            );
                        }
                    })
                    .response
                    .on_hover_text(state.health_summary);

                    if let Some(label) = state.mutation_label {
                        ui.spinner();
                        if !compact {
                            ui.label(RichText::new(label).small().color(theme::MUTED));
                        }
                    }

                    let selector = ui.add_enabled_ui(state.project_enabled, |ui| {
                        egui::ComboBox::from_id_salt("global_project")
                            .selected_text(if state.project.is_empty() {
                                "Все проекты"
                            } else {
                                state.project.as_str()
                            })
                            .width(if compact { 120.0 } else { 145.0 })
                            .show_ui(ui, |ui| {
                                closing_selectable_value(
                                    ui,
                                    state.project,
                                    String::new(),
                                    "Все проекты",
                                );
                                for project in state.projects {
                                    closing_selectable_value(
                                        ui,
                                        state.project,
                                        project.clone(),
                                        project,
                                    );
                                }
                            });
                    });
                    if !state.project_enabled {
                        selector.response.on_disabled_hover_text(
                            "Завершите редактирование или дождитесь активной записи",
                        );
                    }

                    if state.project_loading {
                        ui.spinner();
                    } else if let Some(error) = state.project_error {
                        if ui
                            .small_button("!")
                            .on_hover_text(format!("Список проектов недоступен: {error}"))
                            .clicked()
                        {
                            output.retry_projects = true;
                        }
                    }
                });
            });
        });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_has_a_truthful_layer_label() {
        for route in [
            ShellRoute::Console,
            ShellRoute::Corpus,
            ShellRoute::Search,
            ShellRoute::Graph,
            ShellRoute::Wiki,
            ShellRoute::Agents,
            ShellRoute::Evaluation,
            ShellRoute::Models,
        ] {
            assert!(!route.title().is_empty());
            assert!(!route.layer().is_empty());
        }
    }

    #[test]
    fn only_gateway_workspaces_require_http() {
        assert!(!ShellRoute::Console.requires_http());
        assert!(!ShellRoute::Graph.requires_http());
        assert!(!ShellRoute::Wiki.requires_http());
        assert!(ShellRoute::Corpus.requires_http());
        assert!(ShellRoute::Search.requires_http());
        assert!(ShellRoute::Agents.requires_http());
        assert!(ShellRoute::Evaluation.requires_http());
        assert!(ShellRoute::Models.requires_http());
    }
}
