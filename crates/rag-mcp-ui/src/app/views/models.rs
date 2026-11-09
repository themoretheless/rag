//! Models: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("PIPELINE").monospace().color(theme::L3));
    if app.pending.operations.is_some() {
        ui.spinner();
    } else if let Some(snapshot) = &app.ops.snapshot {
        ui.weak(format!(
            "{} · схема v{} · {} чанков",
            snapshot.status.backend, snapshot.status.schema_version, snapshot.status.chunk_count
        ));
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Runtime и индекс");
        if let Some(snapshot) = &app.ops.snapshot {
            ui.separator();
            ui.label(format!(
                "backend={} schema={} search={}",
                snapshot.status.backend,
                snapshot.status.schema_version,
                if snapshot.status.ready_for_search {
                    "готов"
                } else {
                    "внимание"
                }
            ));
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
    egui::CentralPanel::default().show(root_ui, |ui| {
        let action = draw_models_workspace(
            ui,
            app.ops.snapshot.as_ref(),
            app.ops.error.as_deref(),
            app.pending.operations.is_some(),
            app.ops.snapshot_stale,
        );
        match action {
            ModelsAction::None => {}
            ModelsAction::Refresh => app.reload_operations(),
            ModelsAction::OpenMaintenance => {
                app.mode = ViewMode::Activity;
                app.ops.tab = OperationsTab::Maintenance;
                if app.ops.snapshot.is_none() {
                    app.reload_operations();
                }
            }
        }
    });
}
