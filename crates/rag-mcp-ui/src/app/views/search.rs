//! Search: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    if app.pending.search.is_some() {
        ui.spinner();
        ui.weak("поиск…");
    } else if let Some(results) = &app.search.results {
        ui.weak(format!("{} результатов", results.items.len()));
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Поиск");
        ui.separator();
        ui.label(match app.search.request.mode.as_str() {
            "lex" => "лексический",
            "vec" => "семантический",
            _ => "hybrid",
        });
        if let Some(project) = app.search.request.wing.as_deref() {
            ui.separator();
            ui.label(project);
        }
        if app.pending.search.is_some() {
            ui.separator();
            ui.spinner();
            ui.weak("Поиск…");
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
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
        let action = draw_search_workspace(
            ui,
            &mut app.search.request,
            app.search.results.as_ref(),
            app.search.error.as_deref(),
            app.pending.search.is_some(),
        );
        match action {
            SearchAction::None => {}
            SearchAction::Run => app.run_search(),
            SearchAction::Clear => {
                let wing = nonempty(&app.scope.wing);
                app.search.request = SearchRequest {
                    wing,
                    ..SearchRequest::default()
                };
                app.search.results = None;
                app.search.error = None;
            }
            SearchAction::OpenLibrary {
                document_id,
                title,
                uri,
                include_archived,
            } => {
                app.mode = ViewMode::Library;
                prepare_library_request_from_search(
                    &mut app.library.request,
                    &app.scope.wing,
                    &title,
                    &uri,
                    include_archived,
                );
                app.library.cursor_history.clear();
                app.library.selected_id = None;
                app.library.document = None;
                app.library.document_error = None;
                app.pending.library_document.clear();
                app.library.page = None;
                app.library.open_after_load = Some(document_id);
                app.reload_library();
            }
            SearchAction::OpenGraph { document_id, title } => {
                app.open_graph_focus(document_id, Some(title));
            }
        }
    });
}
