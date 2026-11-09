//! Project Home: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    if app.pending.project_home.is_some() {
        ui.spinner();
        ui.weak("инвентаризация…");
    } else if let Some(home) = &app.home.overview {
        ui.weak(format!("{} документов", home.documents));
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Пульт проекта");
        ui.separator();
        if let Some(project) = nonempty(&app.scope.wing) {
            ui.label(project);
        } else {
            ui.colored_label(egui::Color32::from_rgb(220, 150, 65), "выберите проект");
        }
        if app.pending.project_home.is_some() {
            ui.separator();
            ui.spinner();
            ui.weak("Обновление…");
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, ctx: &egui::Context) {
    egui::CentralPanel::default().show(root_ui, |ui| {
        if app.graph.full_view.is_none() && app.activity_base().is_none() {
            if app.pending.graph.is_some() {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.spinner();
                    ui.label("Подключение к базе знаний…");
                });
            } else {
                match draw_no_source(
                    ui,
                    &mut app.connect_url,
                    app.graph.load_error.as_deref(),
                    app.open.source.is_some(),
                ) {
                    NoSourceAction::Connect => app.connect_http(),
                    NoSourceAction::Retry => app.dispatch_graph_load(),
                    NoSourceAction::None => {}
                }
            }
            return;
        }
        let action = draw_project_home(
            ui,
            ProjectHomeView {
                selected_project: nonempty(&app.scope.wing).as_deref(),
                home: app.home.overview.as_ref(),
                operations: app.ops.snapshot.as_ref(),
                activity: &app.activity.events,
                project_error: app.home.error.as_deref(),
                operations_error: app.ops.error.as_deref(),
                operations_stale: app.ops.snapshot_stale,
                loading: app.pending.project_home.is_some()
                    || app.pending.operations.is_some()
                    || app.pending.activity.is_some(),
                http_available: app.activity_base().is_some(),
                wiki_available: app.available_load_source().is_some(),
            },
        );
        match action {
            HomeAction::None => {}
            HomeAction::Refresh => {
                app.reload_project_home();
                app.reload_operations();
                app.refresh_activity();
            }
            HomeAction::OpenLibrary => {
                app.activate_shell_route(ShellRoute::Corpus);
            }
            HomeAction::OpenSearch => {
                app.activate_shell_route(ShellRoute::Search);
                ctx.memory_mut(|memory| {
                    memory.request_focus(egui::Id::new("native_search_query"));
                });
            }
            HomeAction::OpenGraph => {
                app.mode = ViewMode::Graph;
                if app.activity_base().is_some() {
                    app.graph.seed_input.clear();
                    app.graph.seed_id = None;
                    app.dispatch_graph_load();
                }
            }
            HomeAction::OpenWiki => {
                app.activate_shell_route(ShellRoute::Wiki);
            }
            HomeAction::OpenAgents => {
                app.activate_shell_route(ShellRoute::Agents);
            }
        }
    });
}
