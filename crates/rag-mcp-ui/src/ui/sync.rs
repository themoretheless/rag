use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::operations::OperationsSnapshot;

use super::theme;

pub fn draw_sync_workspace(
    ui: &mut egui::Ui,
    snapshot: Option<&OperationsSnapshot>,
    error: Option<&str>,
    loading: bool,
) -> bool {
    let mut refresh = false;
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("Синхронизация БД");
                ui.weak("локальная DuckDB ↔ главная DuckDB");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!loading, egui::Button::new("Обновить"))
                    .clicked()
                {
                    refresh = true;
                }
                if loading {
                    ui.spinner();
                }
            });
        });
        ui.add_space(12.0);
        if let Some(error) = error {
            ui.colored_label(theme::DANGER, error);
        }

        let Some(snapshot) = snapshot else {
            ui.weak("Читаю состояние gateway…");
            return;
        };
        let sync = snapshot.sync.as_ref();
        ui.columns(4, |columns| {
            metric(
                &mut columns[0],
                "РОЛЬ",
                sync.map(|s| s.role.as_str()).unwrap_or("—"),
                sync.map(|s| s.node_id.as_str()).unwrap_or("этот процесс"),
            );
            metric(
                &mut columns[1],
                "PRIMARY SEQ",
                &sync
                    .map(|s| s.latest_primary_seq.to_string())
                    .unwrap_or_else(|| "—".into()),
                "канонический курсор",
            );
            metric(
                &mut columns[2],
                "OUTBOX",
                &sync
                    .map(|s| s.pending_outbox.to_string())
                    .unwrap_or_else(|| "—".into()),
                "ждут отправки",
            );
            metric(
                &mut columns[3],
                "УЗЛЫ",
                &sync
                    .map(|s| s.nodes.len().to_string())
                    .unwrap_or_else(|| "0".into()),
                "зарегистрированные БД",
            );
        });
        ui.add_space(12.0);

        let width = ui.available_width().max(620.0);
        let height = 350.0_f32.max(105.0 + sync.map(|s| s.nodes.len()).unwrap_or(0) as f32 * 58.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 12.0, theme::SURFACE);
        painter.rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );

        let primary = Rect::from_center_size(
            Pos2::new(rect.left() + width * 0.23, rect.center().y),
            Vec2::new(245.0, 190.0),
        );
        card(
            &painter,
            primary,
            theme::L2,
            "ГЛАВНАЯ БД",
            snapshot
                .status
                .db_path
                .rsplit('/')
                .next()
                .unwrap_or("rag.duckdb"),
            &format!(
                "schema {} · {} документов",
                snapshot.status.schema_version, snapshot.status.document_count
            ),
        );
        let right_x = rect.left() + width * 0.77;
        let nodes = sync.map(|s| s.nodes.as_slice()).unwrap_or(&[]);
        let count = nodes.len().max(1);
        let start_y = rect.center().y - (count.saturating_sub(1) as f32 * 58.0) / 2.0;
        for index in 0..count {
            let node_rect = Rect::from_center_size(
                Pos2::new(right_x, start_y + index as f32 * 58.0),
                Vec2::new(265.0, 48.0),
            );
            if let Some(node) = nodes.get(index) {
                node_card(
                    &painter,
                    node_rect,
                    node.last_error.is_none(),
                    &node.hostname,
                    &format!(
                        "{} · {} · push {} · pull {}",
                        node.node_id, node.role, node.last_push_seq, node.pull_cursor
                    ),
                );
            } else {
                node_card(
                    &painter,
                    node_rect,
                    false,
                    "Локальная машина",
                    "ожидание регистрации",
                );
            }
        }
        let from = Pos2::new(primary.right() + 8.0, rect.center().y - 18.0);
        let to = Pos2::new(right_x - 145.0, from.y);
        flow(
            &painter,
            from,
            to,
            theme::WARN,
            "push · новые изменения",
            ui.input(|i| i.time),
            false,
        );
        let pull_from = Pos2::new(from.x, rect.center().y + 18.0);
        let pull_to = Pos2::new(to.x, pull_from.y);
        flow(
            &painter,
            pull_from,
            pull_to,
            theme::L2,
            "pull · подтверждённые",
            ui.input(|i| i.time),
            true,
        );

        ui.add_space(12.0);
        ui.label(
            RichText::new(match sync {
                Some(s) if s.role == "primary" => format!(
                    "Главная готова · primary seq {} · реплики появятся после регистрации",
                    s.latest_primary_seq
                ),
                Some(s) => format!(
                    "Фоновый обмен включён · outbox {} · главная {}",
                    s.pending_outbox,
                    s.primary_url.as_deref().unwrap_or("не указана")
                ),
                None => "Gateway не публикует sync status — обновите сервер".into(),
            })
            .monospace()
            .color(if sync.is_some() {
                theme::OK
            } else {
                theme::WARN
            }),
        );
    });
    refresh
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str, hint: &str) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(10.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_height(64.0);
            ui.label(RichText::new(label).small().color(theme::MUTED));
            ui.label(RichText::new(value).size(20.0).strong());
            ui.label(RichText::new(hint).small().color(theme::FAINT));
        });
}

fn card(p: &egui::Painter, rect: Rect, color: Color32, role: &str, name: &str, detail: &str) {
    p.rect_filled(rect, 13.0, theme::rgba(color, 16));
    p.rect_stroke(
        rect,
        13.0,
        Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    p.circle_filled(
        Pos2::new(rect.left() + 18.0, rect.top() + 19.0),
        4.0,
        theme::OK,
    );
    p.text(
        Pos2::new(rect.left() + 29.0, rect.top() + 19.0),
        Align2::LEFT_CENTER,
        role,
        FontId::monospace(10.0),
        color,
    );
    paint_db(
        p,
        Pos2::new(rect.center().x, rect.center().y - 5.0),
        color,
        28.0,
    );
    p.text(
        Pos2::new(rect.center().x, rect.bottom() - 42.0),
        Align2::CENTER_CENTER,
        name,
        FontId::monospace(15.0),
        theme::TEXT,
    );
    p.text(
        Pos2::new(rect.center().x, rect.bottom() - 20.0),
        Align2::CENTER_CENTER,
        detail,
        FontId::monospace(9.0),
        theme::MUTED,
    );
}

fn node_card(p: &egui::Painter, rect: Rect, online: bool, name: &str, detail: &str) {
    p.rect_filled(rect, 9.0, theme::SURFACE);
    p.rect_stroke(
        rect,
        9.0,
        Stroke::new(
            1.0,
            if online {
                theme::rgba(theme::OK, 90)
            } else {
                theme::BORDER
            },
        ),
        egui::StrokeKind::Inside,
    );
    paint_db(
        p,
        Pos2::new(rect.left() + 25.0, rect.center().y),
        theme::MUTED,
        13.0,
    );
    p.text(
        Pos2::new(rect.left() + 49.0, rect.center().y - 7.0),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(12.0),
        theme::TEXT,
    );
    p.text(
        Pos2::new(rect.left() + 49.0, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        detail,
        FontId::monospace(8.5),
        theme::FAINT,
    );
}

fn paint_db(p: &egui::Painter, center: Pos2, color: Color32, radius: f32) {
    let stroke = Stroke::new(1.2, color);
    let box_rect = Rect::from_center_size(center, Vec2::new(radius * 1.4, radius * 1.55));
    p.rect_stroke(box_rect, radius * 0.35, stroke, egui::StrokeKind::Inside);
    for y in [-0.45, 0.0, 0.45] {
        let c = Pos2::new(center.x, center.y + y * radius);
        let points = (0..=24)
            .map(|step| {
                let angle = std::f32::consts::TAU * step as f32 / 24.0;
                Pos2::new(
                    c.x + angle.cos() * radius * 0.7,
                    c.y + angle.sin() * radius * 0.22,
                )
            })
            .collect::<Vec<_>>();
        p.add(egui::Shape::line(points, stroke));
    }
}

fn flow(
    p: &egui::Painter,
    from: Pos2,
    to: Pos2,
    color: Color32,
    label: &str,
    time: f64,
    reverse: bool,
) {
    p.line_segment([from, to], Stroke::new(1.0, theme::rgba(color, 100)));
    p.text(
        Pos2::new((from.x + to.x) / 2.0, from.y - 11.0),
        Align2::CENTER_CENTER,
        label,
        FontId::monospace(8.5),
        color,
    );
    for offset in [0.0, 0.33, 0.66] {
        let mut t = ((time * 0.28 + offset) % 1.0) as f32;
        if reverse {
            t = 1.0 - t;
        }
        p.circle_filled(from.lerp(to, t), 3.0, color);
    }
}
