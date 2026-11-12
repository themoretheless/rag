//! Evaluation: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("EVAL").monospace().color(theme::L1));
    ui.weak("живая telemetry · benchmark-значения не угадываются");
    if app.pending.activity.is_some() {
        ui.spinner();
    } else if ui.button("Обновить telemetry").clicked() {
        app.refresh_activity();
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Оценка");
        ui.separator();
        ui.label(format!("{} событий telemetry", app.activity.events.len()));
        ui.weak("отсутствующие benchmark-метрики показаны как —");
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, ctx: &egui::Context) {
    egui::CentralPanel::default().show(root_ui, |ui| {
        let action = draw_evaluation_workspace(
            ui,
            app.search.results.as_ref(),
            &app.activity.events,
            app.activity.error.as_deref(),
            app.pending.activity.is_some(),
        );
        match action {
            EvaluationAction::None => {}
            EvaluationAction::OpenSearch => {
                app.activate_shell_route(ShellRoute::Search);
                ctx.memory_mut(|memory| {
                    memory.request_focus(egui::Id::new("native_search_query"));
                });
            }
            EvaluationAction::RefreshTelemetry => app.refresh_activity(),
        }
    });
}
