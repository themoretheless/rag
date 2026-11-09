//! Central workspace for the current view mode.

use super::*;

impl GraphApp {
    pub(super) fn draw_workspace(&mut self, root_ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.mode {
            ViewMode::Home => views::home::body(self, root_ui, ctx),
            ViewMode::Library => views::library::body(self, root_ui, ctx),
            ViewMode::Search => views::search::body(self, root_ui, ctx),
            ViewMode::Revisions => views::revisions::body(self, root_ui, ctx),
            ViewMode::Activity => views::activity::body(self, root_ui, ctx),
            ViewMode::Evaluation => views::evaluation::body(self, root_ui, ctx),
            ViewMode::Models => views::models::body(self, root_ui, ctx),
            ViewMode::Sync => views::sync::body(self, root_ui, ctx),
            ViewMode::Wiki => views::wiki::body(self, root_ui, ctx),
            ViewMode::Graph => views::graph::body(self, root_ui, ctx),
        }
    }
}
