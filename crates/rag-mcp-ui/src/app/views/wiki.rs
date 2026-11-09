//! Wiki browser: toolbar, status line and body.

use crate::app::*;

/// Controls in the contextual toolbar strip.
pub(in crate::app) fn toolbar(app: &mut GraphApp, ui: &mut egui::Ui) {
    if ui
        .selectable_label(app.wiki.sidebar_visible, "Страницы")
        .on_hover_text("Показать или скрыть каталог страниц")
        .clicked()
    {
        app.wiki.sidebar_visible = !app.wiki.sidebar_visible;
    }
    if ui
        .add_enabled(!app.wiki.history.is_empty(), egui::Button::new("< Назад"))
        .on_hover_text("Назад по истории страниц (Alt+Left)")
        .clicked()
    {
        app.wiki_go_back();
    }
    if ui.button("Обновить вики").clicked() {
        if app.pending.save.is_some() {
            app.wiki.a.error =
                Some("Идёт сохранение. Дождитесь результата перед обновлением вики.".into());
        } else if app.wiki.edit.as_ref().is_some_and(|e| e.dirty) {
            app.wiki.a.error = Some("Есть несохранённые правки: сохраните или отмените их".into());
        } else {
            app.wiki.edit = None;
            app.wiki.loaded = false;
            app.reload_wiki_catalog();
        }
    }
    if app.pending.catalog.is_some() {
        ui.spinner();
        ui.weak("каталог…");
    }
    if app.pending.page_a.is_some() || app.pending.page_b.is_some() {
        ui.spinner();
        ui.weak("страница…");
    }
    if ui
        .selectable_label(app.wiki.dual_pane, "Две панели")
        .on_hover_text("Две статьи: каталог слева, панель A по центру, B справа")
        .clicked()
    {
        app.wiki.dual_pane = !app.wiki.dual_pane;
        if app.wiki.dual_pane {
            // Next sidebar click fills the empty secondary column.
            app.wiki.focus = WikiFocus::B;
        } else {
            app.clear_wiki_pane_b();
            app.wiki.focus = WikiFocus::A;
        }
    }
    if !app.wiki.dual_pane
        && ui
            .selectable_label(app.wiki.info_visible, "Инфо")
            .on_hover_text("Показать метаданные страницы и обратные ссылки")
            .clicked()
    {
        app.wiki.info_visible = !app.wiki.info_visible;
    }
    if app.wiki.dual_pane {
        if ui
            .selectable_label(app.wiki.focus == WikiFocus::A, "A")
            .on_hover_text("Фокус панели A; редактирование и сохранение работают здесь")
            .clicked()
        {
            app.wiki.focus = WikiFocus::A;
        }
        if ui
            .selectable_label(app.wiki.focus == WikiFocus::B, "B")
            .on_hover_text("Фокус панели B; каталог открывает страницы здесь")
            .clicked()
        {
            app.wiki.focus = WikiFocus::B;
        }
    }
    let editing = app.wiki.edit.is_some();
    if !editing {
        if ui
            .add_enabled(
                app.wiki.a.article.is_some() && app.wiki_can_write(),
                egui::Button::new("Редактировать"),
            )
            .on_hover_text("Редактировать панель A; сохранение идёт через HTTP gateway")
            .clicked()
        {
            app.wiki.focus = WikiFocus::A;
            app.start_wiki_edit();
        }
    } else {
        let saving = app.pending.save.is_some();
        if ui
            .add_enabled(
                app.wiki_can_write() && !saving,
                egui::Button::new("Сохранить"),
            )
            .on_hover_text("Ctrl+S")
            .clicked()
        {
            app.save_wiki_edit();
        }
        if saving {
            ui.spinner();
            ui.weak("сохранение…");
        }
        if ui
            .add_enabled(
                can_cancel_edit(saving),
                egui::Button::new("Отменить правки"),
            )
            .on_hover_text(if saving {
                "Дождитесь результата сохранения"
            } else {
                "Esc"
            })
            .clicked()
        {
            app.cancel_wiki_edit();
        }
    }
    if ui
        .add_enabled(
            app.wiki.a.selected_id.is_some() && !editing,
            egui::Button::new("Показать в графе"),
        )
        .clicked()
    {
        if let Some((id, title)) = app
            .wiki
            .a
            .article
            .as_ref()
            .map(|a| (a.id.clone(), a.title.clone()))
        {
            app.open_graph_focus(id, Some(title));
        }
    }
}

/// Bottom status line.
pub(in crate::app) fn status(app: &mut GraphApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        let (source_color, source_status) = if app.activity_base().is_some() {
            (theme::OK, "Gateway")
        } else {
            match app.source.as_ref() {
                Some(GraphSourceKind::LiveStore { .. }) => (theme::ACCENT, "DB · только чтение"),
                Some(
                    GraphSourceKind::SnapshotFile { .. } | GraphSourceKind::VaultGraphJson { .. },
                ) => (theme::FAINT, "Snapshot · только чтение"),
                Some(GraphSourceKind::HttpService { .. }) => (theme::WARN, "Gateway · неизвестно"),
                None => (theme::DANGER, "Офлайн"),
            }
        };
        ui.colored_label(source_color, source_status).on_hover_text(
            app.source
                .as_ref()
                .map(|source| source.label())
                .unwrap_or("Источник данных не выбран"),
        );
        ui.separator();
        ui.label(format!("{} страниц", app.wiki.pages.len()));
        if app.any_pending() {
            ui.separator();
            ui.spinner();
            ui.weak("Обновление…");
        }
        if app.wiki.dual_pane {
            ui.separator();
            ui.label("Две панели");
        }
        if app.wiki.edit.is_some() {
            ui.separator();
            ui.strong("Редактирование");
            if app.wiki.edit.as_ref().is_some_and(|e| e.dirty) {
                ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "не сохранено");
            }
        }
        if let Some(note) = &app.wiki.save_note {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(100, 180, 120), note);
        }
    });
}

/// Side panels and the central workspace.
pub(in crate::app) fn body(app: &mut GraphApp, root_ui: &mut egui::Ui, _ctx: &egui::Context) {
    // Left: catalog (polished dual-pane nav column).
    if app.wiki.sidebar_visible {
        egui::Panel::left("wiki_nav")
            .default_size(250.0)
            .size_range(190.0..=420.0)
            .resizable(true)
            .show_separator_line(false)
            .frame(theme::toolbar_frame())
            .show(root_ui, |ui| {
                let sidebar_index = app.wiki_sidebar_index();
                let sel = if app.wiki.dual_pane && app.wiki.focus == WikiFocus::B {
                    app.wiki.b.selected_id.as_deref()
                } else {
                    app.wiki.a.selected_id.as_deref()
                };
                if let Some(id) = draw_wiki_sidebar(
                    ui,
                    &app.wiki.pages,
                    &sidebar_index,
                    &mut app.wiki.filter,
                    sel,
                    &mut app.wiki.show_summaries,
                ) {
                    app.open_wiki_page_id(&id);
                }
            });
    }

    // Right: secondary article when dual-pane is on.
    if app.wiki.dual_pane {
        egui::Panel::right("wiki_pane_b")
            .default_size(420.0)
            .size_range(280.0..=900.0)
            .resizable(true)
            .show_separator_line(true)
            .frame(theme::toolbar_frame())
            .show(root_ui, |ui| {
                ui.horizontal(|ui| {
                    let focused = app.wiki.focus == WikiFocus::B;
                    if ui
                        .selectable_label(focused, "Панель B")
                        .on_hover_text("Вторая статья, только чтение")
                        .clicked()
                    {
                        app.wiki.focus = WikiFocus::B;
                    }
                    if app.pending.page_b.is_some() {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if app.wiki.b.selected_id.is_some()
                            && ui
                                .small_button("Очистить")
                                .on_hover_text("Закрыть вторую страницу")
                                .clicked()
                        {
                            app.clear_wiki_pane_b();
                        }
                    });
                });
                ui.separator();
                let known_keys = app.wiki_known_keys();
                let action = draw_wiki_read_view(
                    ui,
                    app.wiki.b.article.as_ref(),
                    app.wiki.b.error.as_deref(),
                    WikiReadContext {
                        known_keys: &known_keys,
                        can_write: false,
                        salt_prefix: "b",
                        loading: app.pending.page_b.is_some(),
                    },
                );
                if action.retry {
                    if let Some(id) = app.wiki.b.selected_id.clone() {
                        app.open_wiki_page_in_b(&id);
                    }
                }
                if action.open_id.is_some() || action.open_link.is_some() {
                    app.wiki.focus = WikiFocus::B;
                }
                if let Some(id) = action.open_id {
                    app.open_wiki_page_id(&id);
                } else if let Some(link) = action.open_link {
                    app.open_wiki_link(&link);
                }
            });
    }

    if !app.wiki.dual_pane && app.wiki.info_visible {
        egui::Panel::right("wiki_info")
            .default_size(260.0)
            .size_range(220.0..=380.0)
            .resizable(true)
            .show_separator_line(false)
            .frame(theme::toolbar_frame())
            .show(root_ui, |ui| {
                let action = draw_wiki_info_panel(
                    ui,
                    app.wiki.a.article.as_ref(),
                    &app.wiki.backlinks,
                    app.wiki.backlinks_error.as_deref(),
                    app.pending.backlinks.is_some(),
                );
                if action.retry {
                    if let Some(id) = app.wiki.a.selected_id.clone() {
                        app.refresh_backlinks(&id);
                    }
                } else if let Some(id) = action.open_id {
                    app.open_wiki_page_id(&id);
                } else if let Some(link) = action.open_link {
                    app.open_wiki_link(&link);
                }
            });
    }

    egui::CentralPanel::default().show(root_ui, |ui| {
        // No source yet (or the initial load failed, e.g. --db on a
        // locked file): same start screen as Graph mode, so Retry
        // re-runs LoadGraph on `open.source` instead of only
        // re-fetching the catalog.
        if app.graph.full_view.is_none() && app.activity_base().is_none() {
            if app.pending.graph.is_some() {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.spinner();
                    ui.label("Загрузка…");
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
        if app.wiki.dual_pane {
            ui.horizontal(|ui| {
                let focused = app.wiki.focus == WikiFocus::A;
                if ui
                    .selectable_label(focused, "Панель A")
                    .on_hover_text("Основная статья; редактирование работает здесь")
                    .clicked()
                {
                    app.wiki.focus = WikiFocus::A;
                }
            });
            ui.separator();
        }
        if app.wiki.dual_pane || !app.wiki.info_visible {
            if app.pending.backlinks.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Обновление обратных ссылок…");
                });
            }
            if let Some(error) = app.wiki.backlinks_error.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(215, 100, 85),
                        format!("Обратные ссылки недоступны: {error}"),
                    );
                    if ui.button("Повторить").clicked() {
                        if let Some(id) = app.wiki.a.selected_id.clone() {
                            app.refresh_backlinks(&id);
                        }
                    }
                });
            }
        }
        let can_write = app.wiki_can_write();
        let saving = app.pending.save.is_some();
        if app.wiki.edit.is_some() && app.wiki.a.article.is_some() {
            // Draw with short-lived field borrows, then handle actions.
            let action = {
                let page = app.wiki.a.article.as_ref().expect("checked above");
                let edit = app.wiki.edit.as_mut().expect("checked above");
                draw_wiki_edit_view(
                    ui,
                    page,
                    app.wiki.a.error.as_deref(),
                    edit,
                    can_write,
                    saving,
                )
            };
            if action.reload {
                app.reload_wiki_page();
            } else if action.save {
                app.save_wiki_edit();
            } else if action.cancel {
                app.cancel_wiki_edit();
            }
        } else {
            let known_keys = app.wiki_known_keys();
            let action = draw_wiki_read_view(
                ui,
                app.wiki.a.article.as_ref(),
                app.wiki.a.error.as_deref(),
                WikiReadContext {
                    known_keys: &known_keys,
                    can_write,
                    salt_prefix: "a",
                    loading: app.pending.page_a.is_some(),
                },
            );
            if action.retry {
                if let Some(id) = app.wiki.a.selected_id.clone() {
                    app.open_wiki_page_id_no_history(&id);
                } else {
                    app.wiki.loaded = false;
                    app.reload_wiki_catalog();
                }
            } else if action.start_edit {
                app.wiki.focus = WikiFocus::A;
                app.start_wiki_edit();
            } else if let Some(id) = action.open_id {
                app.wiki.focus = WikiFocus::A;
                app.open_wiki_page_id(&id);
            } else if let Some(link) = action.open_link {
                app.wiki.focus = WikiFocus::A;
                app.open_wiki_link(&link);
            }
        }
    });
}
