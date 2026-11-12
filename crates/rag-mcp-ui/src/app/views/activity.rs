//! Operations (activity, jobs, maintenance): toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    if ui
        .selectable_label(app.ops.tab == OperationsTab::Activity, "Активность")
        .clicked()
    {
        app.ops.tab = OperationsTab::Activity;
        app.refresh_activity();
    }
    if ui
        .selectable_label(app.ops.tab == OperationsTab::Jobs, "Задачи")
        .clicked()
    {
        app.ops.tab = OperationsTab::Jobs;
        app.reload_jobs();
    }
    if ui
        .selectable_label(
            app.ops.tab == OperationsTab::Maintenance,
            "Диагностика и бэкап",
        )
        .clicked()
    {
        app.ops.tab = OperationsTab::Maintenance;
        if app.ops.snapshot.is_none() {
            app.reload_operations();
        }
    }
    ui.separator();
    if app.ops.tab == OperationsTab::Activity {
        if ui.button("Обновить").clicked() {
            app.refresh_activity();
        }
        ui.checkbox(&mut app.activity.auto_refresh, "Онлайн");
    } else if app.ops.tab == OperationsTab::Jobs {
        if app.pending.jobs.is_some() || app.pending.operation_action.is_some() {
            ui.spinner();
        }
        ui.weak(format!("{} сохранено", app.ops.jobs.len()));
    } else {
        if app.pending.operations.is_some() || app.pending.operation_action.is_some() {
            ui.spinner();
        }
        if app.ops.snapshot_stale {
            ui.colored_label(theme::WARN, "устарело");
        } else if let Some(snapshot) = &app.ops.snapshot {
            ui.colored_label(
                if snapshot.doctor.ok {
                    egui::Color32::from_rgb(90, 190, 125)
                } else {
                    egui::Color32::from_rgb(220, 105, 90)
                },
                if snapshot.doctor.ok {
                    "в норме"
                } else {
                    "требует внимания"
                },
            );
        }
    }
    if app.ops.tab != OperationsTab::Activity {
        return;
    }
    let filters_active = app.activity.kind_filter != "all"
        || app.activity.status_filter != "all"
        || !app.activity.client_filter.is_empty()
        || !app.activity.action_filter.is_empty()
        || !app.activity.filter.is_empty();
    ui.menu_button(
        if filters_active {
            "Фильтры (активны)"
        } else {
            "Фильтры"
        },
        |ui| {
            ui.set_min_width(260.0);
            ui.label("Тип события");
            egui::ComboBox::from_id_salt("activity_kind_filter")
                .selected_text(match app.activity.kind_filter.as_str() {
                    "http" => "HTTP",
                    "mcp_tool" => "MCP tool",
                    _ => "Все типы",
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        ("all", "Все типы"),
                        ("http", "HTTP"),
                        ("mcp_tool", "MCP tool"),
                    ] {
                        crate::ui::closing_selectable_value(
                            ui,
                            &mut app.activity.kind_filter,
                            value.to_string(),
                            label,
                        );
                    }
                });
            ui.label("Результат");
            egui::ComboBox::from_id_salt("activity_status_filter")
                .selected_text(match app.activity.status_filter.as_str() {
                    "success" => "Успех",
                    "error" => "Ошибки",
                    _ => "Все результаты",
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        ("all", "Все результаты"),
                        ("success", "Успех"),
                        ("error", "Ошибки"),
                    ] {
                        crate::ui::closing_selectable_value(
                            ui,
                            &mut app.activity.status_filter,
                            value.to_string(),
                            label,
                        );
                    }
                });
            ui.label("Клиент");
            ui.text_edit_singleline(&mut app.activity.client_filter);
            ui.label("Действие или маршрут");
            ui.text_edit_singleline(&mut app.activity.action_filter);
            ui.label("Любое поле");
            ui.text_edit_singleline(&mut app.activity.filter);
            if ui
                .add_enabled(filters_active, egui::Button::new("Сбросить фильтры"))
                .clicked()
            {
                app.activity.kind_filter = "all".into();
                app.activity.status_filter = "all".into();
                app.activity.client_filter.clear();
                app.activity.action_filter.clear();
                app.activity.filter.clear();
            }
        },
    );
    if app.pending.activity.is_some() {
        ui.spinner();
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Операции");
        ui.separator();
        match app.ops.tab {
            OperationsTab::Activity => {
                ui.label(format!("{} событий сохранено", app.activity.events.len()));
                ui.weak("тела запросов и секретные заголовки не записываются");
            }
            OperationsTab::Jobs => {
                ui.label(format!("{} задач сохранено", app.ops.jobs.len()));
                ui.weak("единая очередь записи");
            }
            OperationsTab::Maintenance => {
                let healthy = app
                    .ops
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.doctor.ok)
                    && !app.ops.snapshot_stale;
                ui.colored_label(
                    if healthy {
                        egui::Color32::from_rgb(90, 190, 125)
                    } else {
                        egui::Color32::from_rgb(220, 150, 65)
                    },
                    if app.ops.snapshot_stale {
                        "состояние устарело"
                    } else if healthy {
                        "в норме"
                    } else {
                        "проверьте диагностику"
                    },
                );
            }
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
    match app.ops.tab {
        OperationsTab::Activity => {
            egui::CentralPanel::default().show(root_ui, |ui| {
            ui.heading("Операции · Журнал");
            ui.label("Сначала новые события. Клиенты используют стабильные анонимные ID; тела, секретные заголовки и результаты здесь не записываются.");
            if let Some(error) = &app.activity.error {
                ui.colored_label(theme::DANGER, error);
            }
            ui.separator();
            let filter = app.activity.filter.trim().to_lowercase();
            let client_filter = app.activity.client_filter.trim().to_lowercase();
            let action_filter = app.activity.action_filter.trim().to_lowercase();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("activity_grid")
                    .striped(true)
                    .min_col_width(70.0)
                    .show(ui, |ui| {
                        ui.strong("Локальное время");
                        ui.strong("Тип");
                        ui.strong("Клиент");
                        ui.strong("Действие");
                        ui.strong("Статус");
                        ui.strong("Длительность");
                        ui.strong("ID запроса");
                        ui.end_row();
                        for (event, key) in app.activity.events.iter().zip(&app.activity.search_keys).rev() {
                            if app.activity.kind_filter != "all"
                                && event.kind != app.activity.kind_filter
                            {
                                continue;
                            }
                            let status_matches = match app.activity.status_filter.as_str() {
                                "success" => event.status.is_some_and(|status| status < 400),
                                "error" => event.status.is_some_and(|status| status >= 400),
                                _ => true,
                            };
                            if !status_matches {
                                continue;
                            }
                            if !client_filter.is_empty() && !key.client.contains(&client_filter) {
                                continue;
                            }
                            if !action_filter.is_empty() && !key.action.contains(&action_filter) {
                                continue;
                            }
                            if !filter.is_empty() && !key.any.contains(&filter) {
                                continue;
                            }
                            let local_time = event.at.get(..19).unwrap_or(&event.at).replace('T', " ");
                            ui.monospace(local_time);
                            ui.label(event.kind.as_ref());
                            ui.monospace(event.client.as_deref().unwrap_or("—"));
                            ui.label(&event.action)
                                .on_hover_text(format!("событие #{}", event.seq));
                            let status = event.status.map(|value| value.to_string()).unwrap_or_else(|| "—".into());
                            let status_color = match event.status {
                                Some(value) if value >= 400 => theme::DANGER,
                                Some(_) => theme::OK,
                                None => theme::FAINT,
                            };
                            ui.colored_label(
                                status_color,
                                status,
                            );
                            ui.monospace(
                                event
                                    .elapsed_ms
                                    .map(|ms| format!("{ms:.1} мс"))
                                    .unwrap_or_else(|| "—".into()),
                            );
                            ui.monospace(event.request_id.as_deref().unwrap_or("—"));
                            ui.end_row();
                        }
                    });
            });
        });
        }
        OperationsTab::Jobs => {
            let mut action = OperationsAction::None;
            egui::CentralPanel::default().show(root_ui, |ui| {
                action = draw_jobs(
                    ui,
                    &app.ops.jobs,
                    &mut app.ops.sync_job_form,
                    nonempty(&app.scope.wing).as_deref(),
                    app.ops.error.as_deref(),
                    app.pending.jobs.is_some() || app.pending.operation_action.is_some(),
                );
            });
            app.dispatch_operations_action(action);
        }
        OperationsTab::Maintenance => {
            let mut action = OperationsAction::None;
            egui::CentralPanel::default().show(root_ui, |ui| {
                action = draw_maintenance(
                    ui,
                    app.ops.snapshot.as_ref(),
                    &mut app.ops.backup_form,
                    app.ops.last_result.as_ref(),
                    app.ops.error.as_deref(),
                    app.ops.snapshot_stale,
                    app.pending.operations.is_some() || app.pending.operation_action.is_some(),
                );
            });
            app.dispatch_operations_action(action);
        }
    }
}
