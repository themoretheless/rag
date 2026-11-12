//! Per-frame panels around the workspace: contextual toolbar and status bar.

use super::*;

impl GraphApp {
    pub(super) fn draw_workspace_toolbar(&mut self, root_ui: &mut egui::Ui) {
        if matches!(
            self.mode,
            ViewMode::Activity | ViewMode::Wiki | ViewMode::Graph
        ) {
            egui::Panel::top("workspace_toolbar")
                .resizable(false)
                .frame(theme::toolbar_frame())
                .show(root_ui, |ui| {
                    ui.horizontal_wrapped(|ui| match self.mode {
                        ViewMode::Home => views::home::toolbar(self, ui),
                        ViewMode::Library => views::library::toolbar(self, ui),
                        ViewMode::Search => views::search::toolbar(self, ui),
                        ViewMode::Revisions => views::revisions::toolbar(self, ui),
                        ViewMode::Activity => views::activity::toolbar(self, ui),
                        ViewMode::Evaluation => views::evaluation::toolbar(self, ui),
                        ViewMode::Models => views::models::toolbar(self, ui),
                        ViewMode::Sync => views::sync::toolbar(self, ui),
                        ViewMode::Wiki => views::wiki::toolbar(self, ui),
                        ViewMode::Graph => views::graph::toolbar(self, ui),
                    });
                });
        }
    }

    pub(super) fn draw_status_bar(&mut self, root_ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .resizable(false)
            .frame(theme::toolbar_frame())
            .show(root_ui, |ui| match self.mode {
                ViewMode::Home => views::home::status(self, ui),
                ViewMode::Library => views::library::status(self, ui),
                ViewMode::Search => views::search::status(self, ui),
                ViewMode::Revisions => views::revisions::status(self, ui),
                ViewMode::Activity => views::activity::status(self, ui),
                ViewMode::Evaluation => views::evaluation::status(self, ui),
                ViewMode::Models => views::models::status(self, ui),
                ViewMode::Sync => views::sync::status(self, ui),
                ViewMode::Wiki => views::wiki::status(self, ui),
                ViewMode::Graph => views::graph::status(self, ui),
            });
    }
}
