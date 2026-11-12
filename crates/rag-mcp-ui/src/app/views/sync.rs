//! Sync: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("SYNC").monospace().color(theme::L2));
    if let Some(sync) = app
        .ops
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.sync.as_ref())
    {
        ui.weak(format!(
            "{} · cursor {} · outbox {}",
            sync.role, sync.latest_primary_seq, sync.pending_outbox
        ));
    } else {
        ui.weak("ожидание sync status");
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Синхронизация БД");
        if let Some(sync) = app
            .ops
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.sync.as_ref())
        {
            ui.separator();
            ui.label(format!(
                "{} · primary seq {} · outbox {}",
                sync.role, sync.latest_primary_seq, sync.pending_outbox
            ));
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
    egui::CentralPanel::default().show(root_ui, |ui| {
        if draw_sync_workspace(
            ui,
            app.ops.snapshot.as_ref(),
            app.ops.error.as_deref(),
            app.pending.operations.is_some(),
        ) {
            app.reload_operations();
        }
    });
}
