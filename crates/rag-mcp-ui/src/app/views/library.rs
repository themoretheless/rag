//! Unified Library: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    if app.pending.library.is_some() {
        ui.spinner();
        ui.weak("каталог…");
    } else if let Some(page) = &app.library.page {
        ui.weak(format!("{} документов", page.total));
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Корпус");
        if let Some(page) = &app.library.page {
            ui.separator();
            ui.label(format!("{} всего", page.total));
            ui.label(format!("{} на странице", page.items.len()));
        }
        if app.pending.library.is_some() || app.pending.library_document.is_some() {
            ui.separator();
            ui.spinner();
            ui.weak("Обновление…");
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
    if let Some(item) = app.selected_library_item() {
        let mut detail_action = LibraryDetailAction::None;
        egui::Panel::right("unified_library_detail")
            .default_size(480.0)
            .size_range(360.0..=760.0)
            .resizable(true)
            .frame(theme::toolbar_frame())
            .show(root_ui, |ui| {
                detail_action = draw_library_detail(
                    ui,
                    &item,
                    app.library.document.as_ref(),
                    app.library.document_error.as_deref(),
                    app.pending.library_document.is_some(),
                );
            });
        match detail_action {
            LibraryDetailAction::None => {}
            LibraryDetailAction::Close => {
                app.library.selected_id = None;
                app.library.document = None;
                app.library.document_error = None;
                app.pending.library_document.clear();
            }
            LibraryDetailAction::Retry => {
                let id = item.id.clone();
                app.select_library_document(&id);
            }
            LibraryDetailAction::History(_) => app.open_revisions(&item),
            LibraryDetailAction::OpenWiki(id) => {
                app.mode = ViewMode::Wiki;
                if app.wiki.pages.iter().any(|page| page.id == id) {
                    app.open_wiki_page_id(&id);
                } else {
                    app.wiki.open_after_catalog = Some(id);
                    app.wiki.loaded = false;
                    app.reload_wiki_catalog();
                }
            }
            LibraryDetailAction::OpenGraph(id) => {
                app.open_graph_focus(id, Some(item.title));
            }
        }
    }

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
        let action = draw_library_workspace(
            ui,
            &mut app.library.request,
            app.library.page.as_ref(),
            app.library.selected_id.as_deref(),
            app.library.error.as_deref(),
            app.pending.library.is_some(),
            !app.library.cursor_history.is_empty(),
        );
        match action {
            LibraryAction::None => {}
            LibraryAction::ApplyFilters => app.reset_library_for_filters(),
            LibraryAction::ResetFilters => {
                app.library.request.clear_filters();
                app.reset_library_for_filters();
            }
            LibraryAction::Refresh => app.reload_library(),
            LibraryAction::PreviousPage => {
                if let Some(cursor) = app.library.cursor_history.pop() {
                    app.library.request.cursor = cursor;
                    app.reload_library();
                }
            }
            LibraryAction::NextPage(cursor) => {
                app.library
                    .cursor_history
                    .push(app.library.request.cursor.clone());
                app.library.request.cursor = Some(cursor);
                app.reload_library();
            }
            LibraryAction::Select(id) => app.select_library_document(&id),
        }
    });
}
