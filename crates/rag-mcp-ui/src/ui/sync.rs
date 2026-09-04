use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::collections::HashSet;

use crate::operations::OperationsSnapshot;

use super::theme;

pub fn draw_sync_workspace(
    ui: &mut egui::Ui,
    snapshot: Option<&OperationsSnapshot>,
    error: Option<&str>,
    loading: bool,
) -> bool {
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let mut refresh = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        header(ui, loading, &mut refresh);
        if let Some(error) = error {
            error_banner(ui, error);
        }

        let Some(snapshot) = snapshot else {
            loading_card(ui);
            return;
        };
        let sync = snapshot.sync.as_ref();
        let visible_nodes = observed_nodes(snapshot);

        ui.add_space(14.0);
        ui.columns(4, |columns| {
            metric(
                &mut columns[0],
                "РОЛЬ",
                sync.map(|s| s.role.as_str()).unwrap_or("—"),
                sync.map(|s| s.node_id.as_str()).unwrap_or("этот процесс"),
                theme::L2,
            );
            metric(
                &mut columns[1],
                "PRIMARY SEQ",
                &sync
                    .map(|s| s.latest_primary_seq.to_string())
                    .unwrap_or_else(|| "—".into()),
                "канонический курсор",
                theme::TEXT,
            );
            metric(
                &mut columns[2],
                "OUTBOX",
                &sync
                    .map(|s| s.pending_outbox.to_string())
                    .unwrap_or_else(|| "—".into()),
                "ждут отправки",
                if sync.is_some_and(|s| s.pending_outbox > 0) {
                    theme::WARN
                } else {
                    theme::OK
                },
            );
            metric(
                &mut columns[3],
                "УЗЛЫ",
                &visible_nodes.len().to_string(),
                "клиенты и sync-узлы",
                theme::L3,
            );
        });
        ui.add_space(12.0);

        topology(ui, snapshot);
        ui.add_space(12.0);
        stages(ui, sync.is_some());
        ui.add_space(8.0);
    });
    refresh
}

fn header(ui: &mut egui::Ui, loading: bool, refresh: &mut bool) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new("СИНХРОНИЗАЦИЯ")
                    .monospace()
                    .small()
                    .color(theme::L2),
            );
            ui.heading("Базы данных");
            ui.label(
                RichText::new("Каждая машина пишет локально, главная собирает и раздаёт изменения")
                    .color(theme::MUTED),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!loading, egui::Button::new("  Обновить  "))
                .clicked()
            {
                *refresh = true;
            }
            if loading {
                ui.spinner();
            } else {
                ui.label(
                    RichText::new("●  LIVE")
                        .monospace()
                        .small()
                        .color(theme::OK),
                );
            }
        });
    });
}

fn error_banner(ui: &mut egui::Ui, error: &str) {
    ui.add_space(10.0);
    egui::Frame::new()
        .fill(theme::rgba(theme::DANGER, 12))
        .stroke(Stroke::new(1.0, theme::rgba(theme::DANGER, 90)))
        .corner_radius(8.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.colored_label(theme::DANGER, error);
        });
}

fn loading_card(ui: &mut egui::Ui) {
    ui.add_space(20.0);
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(12.0)
        .inner_margin(24.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Читаю состояние gateway…");
            });
        });
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str, hint: &str, accent: Color32) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(11.0)
        .inner_margin(13.0)
        .show(ui, |ui| {
            ui.set_min_height(70.0);
            ui.label(RichText::new(label).monospace().small().color(theme::MUTED));
            ui.label(RichText::new(value).size(22.0).strong().color(accent));
            ui.label(RichText::new(hint).monospace().small().color(theme::FAINT));
        });
}

fn topology(ui: &mut egui::Ui, snapshot: &OperationsSnapshot) {
    let sync = snapshot.sync.as_ref();
    let nodes = observed_nodes(snapshot);
    let width = ui.available_width().max(720.0);
    let canvas_height = 430.0_f32.max(130.0 + nodes.len().max(1) as f32 * 66.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, canvas_height + 82.0), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 13.0, theme::SURFACE);
    painter.rect_stroke(
        rect,
        13.0,
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    let head = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + 46.0));
    painter.line_segment(
        [head.left_bottom(), head.right_bottom()],
        Stroke::new(1.0, theme::BORDER),
    );
    painter.text(
        Pos2::new(head.left() + 15.0, head.center().y),
        Align2::LEFT_CENTER,
        "ЖИВАЯ ТОПОЛОГИЯ",
        FontId::proportional(12.0),
        theme::TEXT,
    );
    painter.text(
        Pos2::new(head.left() + 145.0, head.center().y),
        Align2::LEFT_CENTER,
        "local → outbox → primary → pull",
        FontId::monospace(9.0),
        theme::FAINT,
    );
    painter.circle_filled(
        Pos2::new(head.right() - 78.0, head.center().y),
        3.5,
        theme::OK,
    );
    painter.text(
        Pos2::new(head.right() - 68.0, head.center().y),
        Align2::LEFT_CENTER,
        "обновляется",
        FontId::monospace(8.5),
        theme::FAINT,
    );

    let canvas = Rect::from_min_max(
        Pos2::new(rect.left(), head.bottom()),
        Pos2::new(rect.right(), rect.bottom() - 36.0),
    );
    paint_grid(&painter, canvas);

    let primary = Rect::from_center_size(
        Pos2::new(canvas.left() + width * 0.22, canvas.center().y),
        Vec2::new(270.0, 238.0),
    );
    primary_card(&painter, primary, snapshot);

    let right_x = canvas.left() + width * 0.80;
    let count = nodes.len().max(1);
    let start_y = canvas.center().y - (count.saturating_sub(1) as f32 * 70.0) / 2.0;
    for index in 0..count {
        let node_rect = Rect::from_center_size(
            Pos2::new(right_x, start_y + index as f32 * 70.0),
            Vec2::new(275.0, 58.0),
        );
        if let Some(node) = nodes.get(index) {
            node_card(
                &painter,
                node_rect,
                node.online,
                &node.label,
                &node.detail,
                &node.badge,
            );
        } else {
            node_card(
                &painter,
                node_rect,
                false,
                "Новая машина",
                "ещё не подключена",
                "ожидание регистрации",
            );
        }
    }

    let line_left = primary.right() + 10.0;
    let line_right = right_x - 148.0;
    let now = ui.input(|i| i.time);
    flow(
        &painter,
        Pos2::new(line_left, canvas.center().y - 21.0),
        Pos2::new(line_right, canvas.center().y - 21.0),
        theme::WARN,
        "PUSH  ·  НОВЫЕ ИЗМЕНЕНИЯ",
        now,
        false,
    );
    flow(
        &painter,
        Pos2::new(line_left, canvas.center().y + 21.0),
        Pos2::new(line_right, canvas.center().y + 21.0),
        theme::L2,
        "PULL  ·  ПОДТВЕРЖДЁННЫЕ",
        now,
        true,
    );

    let footer_y = rect.bottom() - 18.0;
    painter.circle_filled(Pos2::new(rect.left() + 16.0, footer_y), 3.5, theme::OK);
    let truth = match sync {
        Some(s) if s.role == "primary" => format!(
            "Главная готова  ·  primary seq {}  ·  реплики появятся после регистрации",
            s.latest_primary_seq
        ),
        Some(s) => format!(
            "Фоновый обмен включён  ·  outbox {}  ·  главная {}",
            s.pending_outbox,
            s.primary_url.as_deref().unwrap_or("не указана")
        ),
        None => "Gateway пока не публикует sync status".into(),
    };
    painter.text(
        Pos2::new(rect.left() + 28.0, footer_y),
        Align2::LEFT_CENTER,
        truth,
        FontId::monospace(9.0),
        theme::MUTED,
    );
}

fn paint_grid(p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 0.0, theme::rgba(theme::BG, 150));
    let grid = theme::rgba(theme::BORDER, 42);
    let mut x = rect.left() + 24.0;
    while x < rect.right() {
        p.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(0.5, grid),
        );
        x += 32.0;
    }
    let mut y = rect.top() + 24.0;
    while y < rect.bottom() {
        p.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(0.5, grid),
        );
        y += 32.0;
    }
}

fn primary_card(p: &egui::Painter, rect: Rect, snapshot: &OperationsSnapshot) {
    p.rect_filled(rect.expand(16.0), 24.0, theme::rgba(theme::L2, 5));
    p.rect_filled(rect, 15.0, theme::rgba(theme::L2, 18));
    p.rect_stroke(
        rect,
        15.0,
        Stroke::new(1.2, theme::rgba(theme::L2, 200)),
        egui::StrokeKind::Inside,
    );
    p.circle_filled(
        Pos2::new(rect.left() + 18.0, rect.top() + 20.0),
        4.0,
        theme::OK,
    );
    p.text(
        Pos2::new(rect.left() + 30.0, rect.top() + 20.0),
        Align2::LEFT_CENTER,
        "ГЛАВНАЯ БД",
        FontId::monospace(9.0),
        theme::L2,
    );
    paint_db(
        p,
        Pos2::new(rect.center().x, rect.center().y - 15.0),
        theme::L2,
        31.0,
    );
    p.text(
        Pos2::new(rect.center().x, rect.bottom() - 72.0),
        Align2::CENTER_CENTER,
        snapshot
            .status
            .db_path
            .rsplit('/')
            .next()
            .unwrap_or("rag.duckdb"),
        FontId::monospace(15.0),
        theme::TEXT,
    );
    p.text(
        Pos2::new(rect.center().x, rect.bottom() - 49.0),
        Align2::CENTER_CENTER,
        elide_middle(&snapshot.status.db_path, 38),
        FontId::monospace(8.5),
        theme::FAINT,
    );
    p.line_segment(
        [
            Pos2::new(rect.left() + 18.0, rect.bottom() - 31.0),
            Pos2::new(rect.right() - 18.0, rect.bottom() - 31.0),
        ],
        Stroke::new(1.0, theme::BORDER),
    );
    p.text(
        Pos2::new(rect.left() + 18.0, rect.bottom() - 16.0),
        Align2::LEFT_CENTER,
        &snapshot.status.backend,
        FontId::monospace(8.5),
        theme::MUTED,
    );
    p.text(
        Pos2::new(rect.right() - 18.0, rect.bottom() - 16.0),
        Align2::RIGHT_CENTER,
        format!("schema {}", snapshot.status.schema_version),
        FontId::monospace(8.5),
        theme::MUTED,
    );
}

fn node_card(p: &egui::Painter, rect: Rect, online: bool, name: &str, identity: &str, badge: &str) {
    let border = if online {
        theme::rgba(theme::OK, 95)
    } else {
        theme::BORDER
    };
    p.rect_filled(rect, 10.0, theme::rgba(theme::SURFACE, 245));
    p.rect_stroke(
        rect,
        10.0,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    paint_db(
        p,
        Pos2::new(rect.left() + 28.0, rect.center().y),
        theme::MUTED,
        13.0,
    );
    p.text(
        Pos2::new(rect.left() + 52.0, rect.center().y - 11.0),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(12.0),
        theme::TEXT,
    );
    p.text(
        Pos2::new(rect.left() + 52.0, rect.center().y + 5.0),
        Align2::LEFT_CENTER,
        identity,
        FontId::monospace(8.0),
        theme::FAINT,
    );
    p.text(
        Pos2::new(rect.right() - 10.0, rect.center().y + 5.0),
        Align2::RIGHT_CENTER,
        badge,
        FontId::monospace(8.0),
        theme::WARN,
    );
}

struct ObservedNode {
    label: String,
    detail: String,
    badge: String,
    online: bool,
}

fn observed_nodes(snapshot: &OperationsSnapshot) -> Vec<ObservedNode> {
    let mut result = Vec::new();
    let mut ids = HashSet::new();
    if let Some(sync) = &snapshot.sync {
        for node in &sync.nodes {
            ids.insert(node.node_id.to_lowercase());
            ids.insert(node.hostname.to_lowercase());
            result.push(ObservedNode {
                label: node.hostname.clone(),
                detail: format!(
                    "{} · push {} · pull {}",
                    node.role, node.last_push_seq, node.pull_cursor
                ),
                badge: "SYNC NODE".into(),
                online: node.last_error.is_none(),
            });
        }
    }
    for activity in snapshot.activity.iter().rev() {
        let Some(raw) = activity
            .client
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if !ids.insert(raw.to_lowercase()) {
            continue;
        }
        let (label, detail, badge) = if let Some(host) = raw.strip_prefix("host:") {
            (host, "хост замечен в запросах", "LOCAL DB: НЕ ПОДТВЕРЖДЕНА")
        } else {
            (raw, "MCP-клиент", "CLIENT")
        };
        result.push(ObservedNode {
            label: label.into(),
            detail: detail.into(),
            badge: badge.into(),
            online: true,
        });
        if result.len() == 5 {
            return result;
        }
    }
    for agent in &snapshot.agents {
        if !ids.insert(agent.agent.to_lowercase()) {
            continue;
        }
        result.push(ObservedNode {
            label: agent.agent.clone(),
            detail: agent
                .transport
                .clone()
                .unwrap_or_else(|| "MCP-клиент".into()),
            badge: "CLIENT".into(),
            online: agent.online,
        });
        if result.len() == 5 {
            break;
        }
    }
    result
}

fn elide_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    let side = (max_chars.saturating_sub(1)) / 2;
    format!(
        "{}…{}",
        chars[..side].iter().collect::<String>(),
        chars[chars.len() - side..].iter().collect::<String>()
    )
}

fn stages(ui: &mut egui::Ui, live: bool) {
    let items = [
        ("01", "Локальная запись", "Своя DuckDB принимает изменения"),
        ("02", "Outbox -> primary", "Доставка с node_id + seq"),
        ("03", "Primary решает", "Порядок и конфликты"),
        ("04", "Pull -> local", "Курсор и локальный rebuild"),
    ];
    ui.columns(4, |columns| {
        for (column, (number, title, detail)) in columns.iter_mut().zip(items) {
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(10.0)
                .inner_margin(11.0)
                .show(column, |ui| {
                    ui.set_min_height(55.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(number).monospace().color(theme::L2));
                        ui.vertical(|ui| {
                            ui.strong(title);
                            ui.label(RichText::new(detail).small().color(theme::FAINT));
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if live { "ГОТОВО" } else { "ПЛАН" })
                                .monospace()
                                .small()
                                .color(if live { theme::OK } else { theme::WARN }),
                        );
                    });
                });
        }
    });
}

fn paint_db(p: &egui::Painter, center: Pos2, color: Color32, radius: f32) {
    let stroke = Stroke::new(1.2, color);
    for y in [-0.48, 0.0, 0.48] {
        let c = Pos2::new(center.x, center.y + y * radius);
        let points = (0..=28)
            .map(|step| {
                let angle = std::f32::consts::TAU * step as f32 / 28.0;
                Pos2::new(
                    c.x + angle.cos() * radius * 0.72,
                    c.y + angle.sin() * radius * 0.23,
                )
            })
            .collect::<Vec<_>>();
        p.add(egui::Shape::line(points, stroke));
    }
    for x in [-0.72, 0.72] {
        p.line_segment(
            [
                Pos2::new(center.x + radius * x, center.y - radius * 0.48),
                Pos2::new(center.x + radius * x, center.y + radius * 0.48),
            ],
            stroke,
        );
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
    let (dash, gap, length) = (8.0, 6.0, (to.x - from.x).max(0.0));
    let mut x = 0.0;
    while x < length {
        p.line_segment(
            [
                Pos2::new(from.x + x, from.y),
                Pos2::new((from.x + x + dash).min(to.x), from.y),
            ],
            Stroke::new(1.0, theme::rgba(color, 105)),
        );
        x += dash + gap;
    }
    p.text(
        Pos2::new((from.x + to.x) / 2.0, from.y - 13.0),
        Align2::CENTER_CENTER,
        label,
        FontId::monospace(8.0),
        color,
    );
    for offset in [0.0, 0.33, 0.66] {
        let mut t = ((time * 0.34 + offset) % 1.0) as f32;
        if reverse {
            t = 1.0 - t;
        }
        let point = from.lerp(to, t);
        p.circle_filled(point, 6.5, theme::rgba(color, 20));
        p.circle_filled(point, 3.0, color);
    }
}
