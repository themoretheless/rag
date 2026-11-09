//! Connections graph: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    let content_available = app.available_load_source().is_some();
    let previous_lens = app.graph.lens;
    let lens_response = egui::ComboBox::from_id_salt("graph_lens")
        .selected_text(app.graph.lens.label())
        .width(130.0)
        .show_ui(ui, |ui| {
            for lens in GraphLens::ALL {
                ui.selectable_value(&mut app.graph.lens, lens, lens.label());
            }
        });
    lens_response
        .response
        .on_hover_text(app.graph.lens.description());
    if app.graph.lens != previous_lens {
        app.select_node(None, InFlightContent::Keep);
        app.rebuild_ui_graph();
    }
    ui.weak("Фокус");
    let seed_resp = ui.add(
        egui::TextEdit::singleline(&mut app.graph.seed_input)
            .desired_width(180.0)
            .hint_text("Найти узел…"),
    );
    // Seed picker: suggestions over full_view (label / id /
    // document_id substring, case-insensitive, first 10).
    // Cached per (graph revision, query): the scan reruns
    // only when either changes.
    let q = app.graph.seed_input.trim().to_lowercase();
    let suggestions: Vec<(String, String)> = if seed_resp.has_focus() && !q.is_empty() {
        app.seed_suggestions(&q)
    } else {
        Vec::new()
    };
    let mut picked: Option<String> = None;
    if seed_resp.has_focus() && !suggestions.is_empty() {
        egui::Popup::from_response(&seed_resp)
            .id(ui.make_persistent_id("seed_suggestions"))
            .open(true)
            .show(|ui| {
                for (id, label) in &suggestions {
                    if ui
                        .selectable_label(false, format!("{label} · {id}"))
                        .clicked()
                    {
                        picked = Some(id.clone());
                    }
                }
            });
    }
    let first_pick = suggestions.first().map(|(id, _)| id.clone());
    drop(suggestions);
    if seed_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        // Exact id/document/label wins even when an earlier
        // substring suggestion happens to be listed first.
        if let Some(id) = seed_target_on_enter(
            app.graph.full_view.as_ref(),
            &app.graph.seed_input,
            first_pick.as_deref(),
        ) {
            app.graph.seed_input = id;
        }
        app.submit_graph_focus();
    } else if let Some(id) = picked {
        seed_resp.surrender_focus();
        app.graph.seed_input = id;
        app.submit_graph_focus();
    }
    if ui.button("Показать").clicked() {
        app.submit_graph_focus();
    }
    ui.menu_button("Вид", |ui| {
        ui.set_min_width(220.0);
        ui.horizontal(|ui| {
            ui.label("Глубина связей");
            let mut d = app.graph.depth as i32;
            if ui.add(egui::DragValue::new(&mut d).range(1..=3)).changed() {
                app.graph.depth = d as u32;
                if app.topology_query_requires_reload() {
                    app.dispatch_graph_load();
                } else {
                    app.rebuild_ui_graph();
                }
            }
        });
        ui.checkbox(&mut app.graph.show_tags, "Показывать теги");
        ui.checkbox(&mut app.graph.show_stubs, "Показывать заглушки");
        ui.separator();
        ui.label("Комната");
        ui.add(
            egui::TextEdit::singleline(&mut app.scope.room)
                .desired_width(f32::INFINITY)
                .hint_text("Все комнаты"),
        );
        if ui
            .add_enabled(
                !app.scope.room.is_empty(),
                egui::Button::new("Сбросить фильтр комнаты"),
            )
            .clicked()
        {
            app.scope.room.clear();
        }
    });
    if ui
        .button("Сбросить вид")
        .on_hover_text("Перестроить локальный вид от фокуса и убрать добавленные раскрытия")
        .clicked()
    {
        if app.graph.expanded_dirty {
            app.graph.confirm_reset = true;
        } else {
            app.rebuild_ui_graph();
        }
    }
    let expanding = app.pending.expand.is_some();
    if ui
        .add_enabled(
            app.graph.selected.is_some() && !expanding,
            egui::Button::new("Раскрыть"),
        )
        .clicked()
    {
        app.expand_selected();
    }
    if expanding {
        ui.spinner();
    }
    let open_page = ui
        .add_enabled(
            content_available && app.selected_graph_wiki_target().is_some(),
            egui::Button::new("Открыть страницу"),
        )
        .on_hover_text(if content_available {
            "Открыть выбранную wiki-страницу"
        } else {
            "Snapshot содержит только топологию; содержимое требует HTTP или DB"
        });
    if open_page.clicked() {
        app.open_selected_graph_node_in_wiki();
    }
    let preview = ui
        .add_enabled(
            content_available && app.graph.selected.is_some() && app.pending.content.is_none(),
            egui::Button::new("Превью"),
        )
        .on_hover_text(if content_available {
            "Загрузить содержимое выбранного документа"
        } else {
            "Snapshot содержит только топологию; содержимое требует HTTP или DB"
        });
    if preview.clicked() {
        app.load_content_for_selected();
    }
    if app.pending.content.is_some() {
        ui.spinner();
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    let seed_label = app
        .graph
        .seed_id
        .as_deref()
        .or(if app.graph.seed_input.is_empty() {
            None
        } else {
            Some(app.graph.seed_input.as_str())
        });
    let truncated = app
        .graph
        .ui_graph
        .as_ref()
        .is_some_and(|g| g.truncated_nodes || g.truncated_edges)
        || app.graph.raw_truncated
        || app.graph.local_truncated;
    draw_status(
        ui,
        StatusView {
            source: app.source.as_ref(),
            seed_label,
            depth: app.graph.depth,
            graph: app.graph.ui_graph.as_ref(),
            layout_frozen: app.graph.layout_ready,
            truncated,
            banner: app.status_banner().as_deref(),
            pending: app.any_pending(),
        },
    );
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
    if app.graph.selected.is_some() {
        let detail_w = if app.graph.content.is_some() {
            420.0
        } else {
            320.0
        };
        let content_available = app.available_load_source().is_some();
        egui::Panel::right("detail")
            .default_size(detail_w)
            .frame(theme::toolbar_frame())
            .show(root_ui, |ui| {
                if let (Some(sel), Some(g), Some(scene)) = (
                    app.graph.selected.as_deref(),
                    app.graph.ui_graph.as_ref(),
                    app.graph.scene.as_ref(),
                ) {
                    let action = draw_detail(
                        ui,
                        g,
                        scene,
                        sel,
                        app.graph.lens,
                        DetailContentState {
                            body: app.graph.content.as_ref(),
                            error: app.graph.content_error.as_deref(),
                            loading: app.pending.content.is_some(),
                            available: content_available,
                        },
                    );
                    match action {
                        DetailAction::ReadContent => app.load_content_for_selected(),
                        DetailAction::CloseContent => {
                            app.graph.content = None;
                            app.graph.content_error = None;
                        }
                        DetailAction::SelectNode(id) => {
                            app.select_node(Some(id), InFlightContent::Keep);
                        }
                        DetailAction::None => {}
                    }
                }
            });
    }

    egui::CentralPanel::default().show(root_ui, |ui| {
        if app.pending.graph.is_some() && app.graph.full_view.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.2);
                ui.spinner();
                ui.label("Загрузка графа…");
            });
            return;
        }
        if let Some((kind, detail)) = app.empty_kind() {
            match kind {
                // Start screen with actions: Retry + HTTP connect.
                EmptyKind::NoSource | EmptyKind::LoadError => {
                    let can_retry = app.open.source.is_some();
                    match draw_no_source(ui, &mut app.connect_url, detail.as_deref(), can_retry) {
                        NoSourceAction::Retry => app.dispatch_graph_load(),
                        NoSourceAction::Connect => app.connect_http(),
                        NoSourceAction::None => {}
                    }
                }
                _ => {
                    let stats = app.empty_stats();
                    draw_empty_banner(ui, kind, detail.as_deref(), stats.as_ref());
                }
            }
            return;
        }
        let (Some(graph), Some(scene)) = (app.graph.ui_graph.as_ref(), app.graph.scene.as_ref())
        else {
            let stats = app.empty_stats();
            draw_empty_banner(ui, EmptyKind::MissingSeed, None, stats.as_ref());
            return;
        };
        let out = draw_canvas(
            ui,
            graph,
            scene,
            app.graph.selected.as_deref(),
            &mut app.graph.pan,
            &mut app.graph.zoom,
            &mut app.graph.need_fit,
        );
        if let Some(id) = out.clicked_id {
            if app.graph.selected.as_deref() != Some(id.as_str()) {
                app.select_node(Some(id), InFlightContent::Cancel);
            }
        } else if out.clicked_empty {
            app.select_node(None, InFlightContent::Cancel);
        }
    });
}
