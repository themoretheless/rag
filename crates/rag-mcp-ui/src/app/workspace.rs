//! Central workspace for the current view mode.

use super::*;

impl GraphApp {
    pub(super) fn draw_workspace(&mut self, root_ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.mode {
            ViewMode::Home => {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    if self.full_view.is_none() && self.activity_base().is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Подключение к базе знаний…");
                            });
                        } else {
                            match draw_no_source(
                                ui,
                                &mut self.connect_url,
                                self.load_error.as_deref(),
                                self.open.source.is_some(),
                            ) {
                                NoSourceAction::Connect => self.connect_http(),
                                NoSourceAction::Retry => self.dispatch_graph_load(),
                                NoSourceAction::None => {}
                            }
                        }
                        return;
                    }
                    let action = draw_project_home(
                        ui,
                        ProjectHomeView {
                            selected_project: nonempty(&self.filter_wing).as_deref(),
                            home: self.project_home.as_ref(),
                            operations: self.operations_snapshot.as_ref(),
                            activity: &self.activity,
                            project_error: self.project_home_error.as_deref(),
                            operations_error: self.operations_error.as_deref(),
                            operations_stale: self.operations_snapshot_stale,
                            loading: self.pending_project_home.is_some()
                                || self.pending_operations.is_some()
                                || self.pending_activity.is_some(),
                            http_available: self.activity_base().is_some(),
                            wiki_available: self.available_load_source().is_some(),
                        },
                    );
                    match action {
                        HomeAction::None => {}
                        HomeAction::Refresh => {
                            self.reload_project_home();
                            self.reload_operations();
                            self.refresh_activity();
                        }
                        HomeAction::OpenLibrary => {
                            self.activate_shell_route(ShellRoute::Corpus);
                        }
                        HomeAction::OpenSearch => {
                            self.activate_shell_route(ShellRoute::Search);
                            ctx.memory_mut(|memory| {
                                memory.request_focus(egui::Id::new("native_search_query"));
                            });
                        }
                        HomeAction::OpenGraph => {
                            self.mode = ViewMode::Graph;
                            if self.activity_base().is_some() {
                                self.seed_input.clear();
                                self.seed_id = None;
                                self.dispatch_graph_load();
                            }
                        }
                        HomeAction::OpenWiki => {
                            self.activate_shell_route(ShellRoute::Wiki);
                        }
                        HomeAction::OpenAgents => {
                            self.activate_shell_route(ShellRoute::Agents);
                        }
                    }
                });
            }
            ViewMode::Library => {
                if let Some(item) = self.selected_library_item() {
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
                                self.library_document.as_ref(),
                                self.library_document_error.as_deref(),
                                self.pending_library_document.is_some(),
                            );
                        });
                    match detail_action {
                        LibraryDetailAction::None => {}
                        LibraryDetailAction::Close => {
                            self.library_selected_id = None;
                            self.library_document = None;
                            self.library_document_error = None;
                            self.pending_library_document = None;
                        }
                        LibraryDetailAction::Retry => {
                            let id = item.id.clone();
                            self.select_library_document(&id);
                        }
                        LibraryDetailAction::History(_) => self.open_revisions(&item),
                        LibraryDetailAction::OpenWiki(id) => {
                            self.mode = ViewMode::Wiki;
                            if self.wiki_pages.iter().any(|page| page.id == id) {
                                self.open_wiki_page_id(&id);
                            } else {
                                self.wiki_open_after_catalog = Some(id);
                                self.wiki_loaded = false;
                                self.reload_wiki_catalog();
                            }
                        }
                        LibraryDetailAction::OpenGraph(id) => {
                            self.open_graph_focus(id, Some(item.title));
                        }
                    }
                }

                egui::CentralPanel::default().show(root_ui, |ui| {
                    if self.full_view.is_none() && self.activity_base().is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Подключение к базе знаний…");
                            });
                        } else {
                            match draw_no_source(
                                ui,
                                &mut self.connect_url,
                                self.load_error.as_deref(),
                                self.open.source.is_some(),
                            ) {
                                NoSourceAction::Connect => self.connect_http(),
                                NoSourceAction::Retry => self.dispatch_graph_load(),
                                NoSourceAction::None => {}
                            }
                        }
                        return;
                    }
                    let action = draw_library_workspace(
                        ui,
                        &mut self.library_request,
                        self.library_page.as_ref(),
                        self.library_selected_id.as_deref(),
                        self.library_error.as_deref(),
                        self.pending_library.is_some(),
                        !self.library_cursor_history.is_empty(),
                    );
                    match action {
                        LibraryAction::None => {}
                        LibraryAction::ApplyFilters => self.reset_library_for_filters(),
                        LibraryAction::ResetFilters => {
                            self.library_request.clear_filters();
                            self.reset_library_for_filters();
                        }
                        LibraryAction::Refresh => self.reload_library(),
                        LibraryAction::PreviousPage => {
                            if let Some(cursor) = self.library_cursor_history.pop() {
                                self.library_request.cursor = cursor;
                                self.reload_library();
                            }
                        }
                        LibraryAction::NextPage(cursor) => {
                            self.library_cursor_history
                                .push(self.library_request.cursor.clone());
                            self.library_request.cursor = Some(cursor);
                            self.reload_library();
                        }
                        LibraryAction::Select(id) => self.select_library_document(&id),
                    }
                });
            }
            ViewMode::Search => {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    if self.full_view.is_none() && self.activity_base().is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Подключение к базе знаний…");
                            });
                        } else {
                            match draw_no_source(
                                ui,
                                &mut self.connect_url,
                                self.load_error.as_deref(),
                                self.open.source.is_some(),
                            ) {
                                NoSourceAction::Connect => self.connect_http(),
                                NoSourceAction::Retry => self.dispatch_graph_load(),
                                NoSourceAction::None => {}
                            }
                        }
                        return;
                    }
                    let action = draw_search_workspace(
                        ui,
                        &mut self.search_request,
                        self.search_results.as_ref(),
                        self.search_error.as_deref(),
                        self.pending_search.is_some(),
                    );
                    match action {
                        SearchAction::None => {}
                        SearchAction::Run => self.run_search(),
                        SearchAction::Clear => {
                            let wing = nonempty(&self.filter_wing);
                            self.search_request = SearchRequest {
                                wing,
                                ..SearchRequest::default()
                            };
                            self.search_results = None;
                            self.search_error = None;
                        }
                        SearchAction::OpenLibrary {
                            document_id,
                            title,
                            uri,
                            include_archived,
                        } => {
                            self.mode = ViewMode::Library;
                            prepare_library_request_from_search(
                                &mut self.library_request,
                                &self.filter_wing,
                                &title,
                                &uri,
                                include_archived,
                            );
                            self.library_cursor_history.clear();
                            self.library_selected_id = None;
                            self.library_document = None;
                            self.library_document_error = None;
                            self.pending_library_document = None;
                            self.library_page = None;
                            self.library_open_after_load = Some(document_id);
                            self.reload_library();
                        }
                        SearchAction::OpenGraph { document_id, title } => {
                            self.open_graph_focus(document_id, Some(title));
                        }
                    }
                });
            }
            ViewMode::Revisions => {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    let action = draw_revisions_workspace(
                        ui,
                        RevisionsView {
                            document_title: &self.revision.document_title,
                            document_uri: &self.revision.document_uri,
                            document_layer: &self.revision.document_layer,
                            head_revision: self.revision.head,
                            revisions: &self.revision.items,
                            total: self.revision.total,
                            next_cursor: self.revision.next_cursor.as_deref(),
                            selected_revision: self.revision.selected,
                            snapshot: self.revision.snapshot.as_ref(),
                            diff: self.revision.diff.as_ref(),
                            restore_result: self.revision.restore_result.as_ref(),
                            confirming_restore: self.revision.restore_confirm,
                            timeline_error: self.revision.error.as_deref(),
                            snapshot_error: self.revision.snapshot_error.as_deref(),
                            diff_error: self.revision.diff_error.as_deref(),
                            loading_timeline: self.pending_revisions.is_some(),
                            loading_snapshot: self.pending_revision_snapshot.is_some(),
                            loading_diff: self.pending_revision_diff.is_some(),
                            loading_restore: self.pending_revision_restore.is_some(),
                        },
                    );
                    match action {
                        RevisionsAction::None => {}
                        RevisionsAction::BackToLibrary => self.mode = ViewMode::Library,
                        RevisionsAction::Refresh => self.reload_revisions(),
                        RevisionsAction::LoadMore => self.load_more_revisions(),
                        RevisionsAction::Select(revision) => self.load_revision_details(revision),
                        RevisionsAction::RequestRestore(revision) => {
                            self.revision.restore_confirm = Some(revision);
                        }
                        RevisionsAction::ConfirmRestore(revision) => {
                            self.restore_revision(revision);
                        }
                        RevisionsAction::CancelRestore => {
                            self.revision.restore_confirm = None;
                        }
                    }
                });
            }
            ViewMode::Activity => match self.operations_tab {
                OperationsTab::Activity => {
                    egui::CentralPanel::default().show(root_ui, |ui| {
                    ui.heading("Операции · Журнал");
                    ui.label("Сначала новые события. Клиенты используют стабильные анонимные ID; тела, секретные заголовки и результаты здесь не записываются.");
                    if let Some(error) = &self.activity_error {
                        ui.colored_label(theme::DANGER, error);
                    }
                    ui.separator();
                    let filter = self.activity_filter.trim().to_lowercase();
                    let client_filter = self.activity_client_filter.trim().to_lowercase();
                    let action_filter = self.activity_action_filter.trim().to_lowercase();
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        egui::Grid::new("activity_grid")
                            .striped(true)
                            .min_col_width(70.0)
                            .show(ui, |ui| {
                                ui.strong("Локальное время");
                                ui.strong("Тип");
                                ui.strong("Клиент");
                                ui.strong("Действие");
                                ui.strong("Статус");
                                ui.strong("Длительность");
                                ui.strong("ID запроса");
                                ui.end_row();
                                for event in self.activity.iter().rev() {
                                    if self.activity_kind_filter != "all"
                                        && event.kind != self.activity_kind_filter
                                    {
                                        continue;
                                    }
                                    let status_matches = match self.activity_status_filter.as_str() {
                                        "success" => event.status.is_some_and(|status| status < 400),
                                        "error" => event.status.is_some_and(|status| status >= 400),
                                        _ => true,
                                    };
                                    if !status_matches {
                                        continue;
                                    }
                                    let client = event.client.as_deref().unwrap_or("");
                                    if !client_filter.is_empty()
                                        && !client.to_lowercase().contains(&client_filter)
                                    {
                                        continue;
                                    }
                                    if !action_filter.is_empty()
                                        && !event.action.to_lowercase().contains(&action_filter)
                                    {
                                        continue;
                                    }
                                    let searchable = format!("{} {} {} {} {}", event.at, event.kind, event.client.as_deref().unwrap_or(""), event.action, event.status.map(|v| v.to_string()).unwrap_or_default()).to_lowercase();
                                    if !filter.is_empty() && !searchable.contains(&filter) {
                                        continue;
                                    }
                                    let local_time = event.at.get(..19).unwrap_or(&event.at).replace('T', " ");
                                    ui.monospace(local_time);
                                    ui.label(event.kind.as_ref());
                                    ui.monospace(event.client.as_deref().unwrap_or("—"));
                                    ui.label(&event.action)
                                        .on_hover_text(format!("событие #{}", event.seq));
                                    let status = event.status.map(|value| value.to_string()).unwrap_or_else(|| "—".into());
                                    let status_color = match event.status {
                                        Some(value) if value >= 400 => theme::DANGER,
                                        Some(_) => theme::OK,
                                        None => theme::FAINT,
                                    };
                                    ui.colored_label(
                                        status_color,
                                        status,
                                    );
                                    ui.monospace(
                                        event
                                            .elapsed_ms
                                            .map(|ms| format!("{ms:.1} мс"))
                                            .unwrap_or_else(|| "—".into()),
                                    );
                                    ui.monospace(event.request_id.as_deref().unwrap_or("—"));
                                    ui.end_row();
                                }
                            });
                    });
                });
                }
                OperationsTab::Jobs => {
                    let mut action = OperationsAction::None;
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        action = draw_jobs(
                            ui,
                            &self.operations_jobs,
                            &mut self.sync_job_form,
                            nonempty(&self.filter_wing).as_deref(),
                            self.operations_error.as_deref(),
                            self.pending_jobs.is_some() || self.pending_operation_action.is_some(),
                        );
                    });
                    self.dispatch_operations_action(action);
                }
                OperationsTab::Maintenance => {
                    let mut action = OperationsAction::None;
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        action = draw_maintenance(
                            ui,
                            self.operations_snapshot.as_ref(),
                            &mut self.backup_form,
                            self.operations_last_result.as_ref(),
                            self.operations_error.as_deref(),
                            self.operations_snapshot_stale,
                            self.pending_operations.is_some()
                                || self.pending_operation_action.is_some(),
                        );
                    });
                    self.dispatch_operations_action(action);
                }
            },
            ViewMode::Evaluation => {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    let action = draw_evaluation_workspace(
                        ui,
                        self.search_results.as_ref(),
                        &self.activity,
                        self.activity_error.as_deref(),
                        self.pending_activity.is_some(),
                    );
                    match action {
                        EvaluationAction::None => {}
                        EvaluationAction::OpenSearch => {
                            self.activate_shell_route(ShellRoute::Search);
                            ctx.memory_mut(|memory| {
                                memory.request_focus(egui::Id::new("native_search_query"));
                            });
                        }
                        EvaluationAction::RefreshTelemetry => self.refresh_activity(),
                    }
                });
            }
            ViewMode::Models => {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    let action = draw_models_workspace(
                        ui,
                        self.operations_snapshot.as_ref(),
                        self.operations_error.as_deref(),
                        self.pending_operations.is_some(),
                        self.operations_snapshot_stale,
                    );
                    match action {
                        ModelsAction::None => {}
                        ModelsAction::Refresh => self.reload_operations(),
                        ModelsAction::OpenMaintenance => {
                            self.mode = ViewMode::Activity;
                            self.operations_tab = OperationsTab::Maintenance;
                            if self.operations_snapshot.is_none() {
                                self.reload_operations();
                            }
                        }
                    }
                });
            }
            ViewMode::Sync => {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    if draw_sync_workspace(
                        ui,
                        self.operations_snapshot.as_ref(),
                        self.operations_error.as_deref(),
                        self.pending_operations.is_some(),
                    ) {
                        self.reload_operations();
                    }
                });
            }
            ViewMode::Wiki => {
                // Left: catalog (polished dual-pane nav column).
                if self.wiki_sidebar_visible {
                    egui::Panel::left("wiki_nav")
                        .default_size(250.0)
                        .size_range(190.0..=420.0)
                        .resizable(true)
                        .show_separator_line(false)
                        .frame(theme::toolbar_frame())
                        .show(root_ui, |ui| {
                            let sidebar_index = self.wiki_sidebar_index();
                            let sel = if self.wiki_dual_pane && self.wiki_focus == WikiPane::B {
                                self.wiki_selected_id_b.as_deref()
                            } else {
                                self.wiki_selected_id.as_deref()
                            };
                            if let Some(id) = draw_wiki_sidebar(
                                ui,
                                &self.wiki_pages,
                                &sidebar_index,
                                &mut self.wiki_filter,
                                sel,
                                &mut self.wiki_show_summaries,
                            ) {
                                self.open_wiki_page_id(&id);
                            }
                        });
                }

                // Right: secondary article when dual-pane is on.
                if self.wiki_dual_pane {
                    egui::Panel::right("wiki_pane_b")
                        .default_size(420.0)
                        .size_range(280.0..=900.0)
                        .resizable(true)
                        .show_separator_line(true)
                        .frame(theme::toolbar_frame())
                        .show(root_ui, |ui| {
                            ui.horizontal(|ui| {
                                let focused = self.wiki_focus == WikiPane::B;
                                if ui
                                    .selectable_label(focused, "Панель B")
                                    .on_hover_text("Вторая статья, только чтение")
                                    .clicked()
                                {
                                    self.wiki_focus = WikiPane::B;
                                }
                                if self.pending_page_b.is_some() {
                                    ui.spinner();
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if self.wiki_selected_id_b.is_some()
                                            && ui
                                                .small_button("Очистить")
                                                .on_hover_text("Закрыть вторую страницу")
                                                .clicked()
                                        {
                                            self.clear_wiki_pane_b();
                                        }
                                    },
                                );
                            });
                            ui.separator();
                            let known_keys = self.wiki_known_keys();
                            let action = draw_wiki_read_view(
                                ui,
                                self.wiki_article_b.as_ref(),
                                self.wiki_error_b.as_deref(),
                                WikiReadContext {
                                    known_keys: &known_keys,
                                    can_write: false,
                                    salt_prefix: "b",
                                    loading: self.pending_page_b.is_some(),
                                },
                            );
                            if action.retry {
                                if let Some(id) = self.wiki_selected_id_b.clone() {
                                    self.open_wiki_page_in_b(&id);
                                }
                            }
                            if action.open_id.is_some() || action.open_link.is_some() {
                                self.wiki_focus = WikiPane::B;
                            }
                            if let Some(id) = action.open_id {
                                self.open_wiki_page_id(&id);
                            } else if let Some(link) = action.open_link {
                                self.open_wiki_link(&link);
                            }
                        });
                }

                if !self.wiki_dual_pane && self.wiki_info_visible {
                    egui::Panel::right("wiki_info")
                        .default_size(260.0)
                        .size_range(220.0..=380.0)
                        .resizable(true)
                        .show_separator_line(false)
                        .frame(theme::toolbar_frame())
                        .show(root_ui, |ui| {
                            let action = draw_wiki_info_panel(
                                ui,
                                self.wiki_article.as_ref(),
                                &self.wiki_backlinks,
                                self.wiki_backlinks_error.as_deref(),
                                self.pending_backlinks.is_some(),
                            );
                            if action.retry {
                                if let Some(id) = self.wiki_selected_id.clone() {
                                    self.refresh_backlinks(&id);
                                }
                            } else if let Some(id) = action.open_id {
                                self.open_wiki_page_id(&id);
                            } else if let Some(link) = action.open_link {
                                self.open_wiki_link(&link);
                            }
                        });
                }

                egui::CentralPanel::default().show(root_ui, |ui| {
                    // No source yet (or the initial load failed, e.g. --db on a
                    // locked file): same start screen as Graph mode, so Retry
                    // re-runs LoadGraph on `open.source` instead of only
                    // re-fetching the catalog.
                    if self.full_view.is_none() && self.activity_base().is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Загрузка…");
                            });
                        } else {
                            match draw_no_source(
                                ui,
                                &mut self.connect_url,
                                self.load_error.as_deref(),
                                self.open.source.is_some(),
                            ) {
                                NoSourceAction::Connect => self.connect_http(),
                                NoSourceAction::Retry => self.dispatch_graph_load(),
                                NoSourceAction::None => {}
                            }
                        }
                        return;
                    }
                    if self.wiki_dual_pane {
                        ui.horizontal(|ui| {
                            let focused = self.wiki_focus == WikiPane::A;
                            if ui
                                .selectable_label(focused, "Панель A")
                                .on_hover_text("Основная статья; редактирование работает здесь")
                                .clicked()
                            {
                                self.wiki_focus = WikiPane::A;
                            }
                        });
                        ui.separator();
                    }
                    if self.wiki_dual_pane || !self.wiki_info_visible {
                        if self.pending_backlinks.is_some() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.weak("Обновление обратных ссылок…");
                            });
                        }
                        if let Some(error) = self.wiki_backlinks_error.clone() {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(
                                    egui::Color32::from_rgb(215, 100, 85),
                                    format!("Обратные ссылки недоступны: {error}"),
                                );
                                if ui.button("Повторить").clicked() {
                                    if let Some(id) = self.wiki_selected_id.clone() {
                                        self.refresh_backlinks(&id);
                                    }
                                }
                            });
                        }
                    }
                    let can_write = self.wiki_can_write();
                    let saving = self.pending_save.is_some();
                    if self.wiki_edit.is_some() && self.wiki_article.is_some() {
                        // Draw with short-lived field borrows, then handle actions.
                        let action = {
                            let page = self.wiki_article.as_ref().expect("checked above");
                            let edit = self.wiki_edit.as_mut().expect("checked above");
                            draw_wiki_edit_view(
                                ui,
                                page,
                                self.wiki_error.as_deref(),
                                edit,
                                can_write,
                                saving,
                            )
                        };
                        if action.reload {
                            self.reload_wiki_page();
                        } else if action.save {
                            self.save_wiki_edit();
                        } else if action.cancel {
                            self.cancel_wiki_edit();
                        }
                    } else {
                        let known_keys = self.wiki_known_keys();
                        let action = draw_wiki_read_view(
                            ui,
                            self.wiki_article.as_ref(),
                            self.wiki_error.as_deref(),
                            WikiReadContext {
                                known_keys: &known_keys,
                                can_write,
                                salt_prefix: "a",
                                loading: self.pending_page_a.is_some(),
                            },
                        );
                        if action.retry {
                            if let Some(id) = self.wiki_selected_id.clone() {
                                self.open_wiki_page_id_no_history(&id);
                            } else {
                                self.wiki_loaded = false;
                                self.reload_wiki_catalog();
                            }
                        } else if action.start_edit {
                            self.wiki_focus = WikiPane::A;
                            self.start_wiki_edit();
                        } else if let Some(id) = action.open_id {
                            self.wiki_focus = WikiPane::A;
                            self.open_wiki_page_id(&id);
                        } else if let Some(link) = action.open_link {
                            self.wiki_focus = WikiPane::A;
                            self.open_wiki_link(&link);
                        }
                    }
                });
            }
            ViewMode::Graph => {
                if self.selected.is_some() {
                    let detail_w = if self.content.is_some() { 420.0 } else { 320.0 };
                    let content_available = self.available_load_source().is_some();
                    egui::Panel::right("detail")
                        .default_size(detail_w)
                        .frame(theme::toolbar_frame())
                        .show(root_ui, |ui| {
                            if let (Some(sel), Some(g)) =
                                (self.selected.as_deref(), self.ui_graph.as_ref())
                            {
                                let action = draw_detail(
                                    ui,
                                    g,
                                    sel,
                                    self.graph_lens,
                                    DetailContentState {
                                        body: self.content.as_ref(),
                                        error: self.content_error.as_deref(),
                                        loading: self.pending_content.is_some(),
                                        available: content_available,
                                    },
                                );
                                match action {
                                    DetailAction::ReadContent => self.load_content_for_selected(),
                                    DetailAction::CloseContent => {
                                        self.content = None;
                                        self.content_error = None;
                                    }
                                    DetailAction::SelectNode(id) => {
                                        self.selected = Some(id);
                                        self.content = None;
                                        self.content_error = None;
                                    }
                                    DetailAction::None => {}
                                }
                            }
                        });
                }

                egui::CentralPanel::default().show(root_ui, |ui| {
                    if self.pending_graph.is_some() && self.full_view.is_none() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(ui.available_height() * 0.2);
                            ui.spinner();
                            ui.label("Загрузка графа…");
                        });
                        return;
                    }
                    if let Some((kind, detail)) = self.empty_kind() {
                        match kind {
                            // Start screen with actions: Retry + HTTP connect.
                            EmptyKind::NoSource | EmptyKind::LoadError => {
                                let can_retry = self.open.source.is_some();
                                match draw_no_source(
                                    ui,
                                    &mut self.connect_url,
                                    detail.as_deref(),
                                    can_retry,
                                ) {
                                    NoSourceAction::Retry => self.dispatch_graph_load(),
                                    NoSourceAction::Connect => self.connect_http(),
                                    NoSourceAction::None => {}
                                }
                            }
                            _ => {
                                let stats = self.empty_stats();
                                draw_empty_banner(ui, kind, detail.as_deref(), stats.as_ref());
                            }
                        }
                        return;
                    }
                    let Some(graph) = self.ui_graph.as_ref() else {
                        let stats = self.empty_stats();
                        draw_empty_banner(ui, EmptyKind::MissingSeed, None, stats.as_ref());
                        return;
                    };
                    let out = draw_canvas(
                        ui,
                        graph,
                        &self.positions,
                        self.selected.as_deref(),
                        &mut self.pan,
                        &mut self.zoom,
                        &mut self.need_fit,
                    );
                    if let Some(id) = out.clicked_id {
                        if self.selected.as_deref() != Some(id.as_str()) {
                            self.content = None;
                            self.content_error = None;
                            self.pending_content = None;
                        }
                        self.selected = Some(id);
                    } else if out.clicked_empty {
                        self.selected = None;
                        self.content = None;
                        self.content_error = None;
                        self.pending_content = None;
                    }
                });
            }
        }
    }
}
