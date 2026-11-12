//! Revision history: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    if app.pending.revisions.is_some()
        || app.pending.revision_snapshot.is_some()
        || app.pending.revision_diff.is_some()
        || app.pending.revision_restore.is_some()
    {
        ui.spinner();
        ui.weak("история…");
    } else if let Some(head) = app.revision.head {
        ui.weak(format!("текущая r{head}"));
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("История версий");
        if let Some(head) = app.revision.head {
            ui.separator();
            ui.label(format!("текущая r{head}"));
        }
        if let Some(selected) = app.revision.selected {
            ui.separator();
            ui.label(format!("сравнение с r{selected}"));
        }
        if app.pending.revisions.is_some()
            || app.pending.revision_snapshot.is_some()
            || app.pending.revision_diff.is_some()
            || app.pending.revision_restore.is_some()
        {
            ui.separator();
            ui.spinner();
            ui.weak("Обновление…");
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
    egui::CentralPanel::default().show(root_ui, |ui| {
        let action = draw_revisions_workspace(
            ui,
            RevisionsView {
                document_title: &app.revision.document_title,
                document_uri: &app.revision.document_uri,
                document_layer: &app.revision.document_layer,
                head_revision: app.revision.head,
                revisions: &app.revision.items,
                total: app.revision.total,
                next_cursor: app.revision.next_cursor.as_deref(),
                selected_revision: app.revision.selected,
                snapshot: app.revision.snapshot.as_ref(),
                diff: app.revision.diff.as_ref(),
                restore_result: app.revision.restore_result.as_ref(),
                confirming_restore: app.revision.restore_confirm,
                timeline_error: app.revision.error.as_deref(),
                snapshot_error: app.revision.snapshot_error.as_deref(),
                diff_error: app.revision.diff_error.as_deref(),
                loading_timeline: app.pending.revisions.is_some(),
                loading_snapshot: app.pending.revision_snapshot.is_some(),
                loading_diff: app.pending.revision_diff.is_some(),
                loading_restore: app.pending.revision_restore.is_some(),
            },
        );
        match action {
            RevisionsAction::None => {}
            RevisionsAction::BackToLibrary => app.mode = ViewMode::Library,
            RevisionsAction::Refresh => app.reload_revisions(),
            RevisionsAction::LoadMore => app.load_more_revisions(),
            RevisionsAction::Select(revision) => app.load_revision_details(revision),
            RevisionsAction::RequestRestore(revision) => {
                app.revision.restore_confirm = Some(revision);
            }
            RevisionsAction::ConfirmRestore(revision) => {
                app.restore_revision(revision);
            }
            RevisionsAction::CancelRestore => {
                app.revision.restore_confirm = None;
            }
        }
    });
}
