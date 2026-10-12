//! Per-frame layout: shell, workspaces, panels and dialogs.

use super::*;

impl eframe::App for GraphApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.drain_worker_events();
        self.intercept_native_close(&ctx);
        self.handle_hotkeys(&ctx);
        if self.mode == ViewMode::Activity
            && self.operations_tab == OperationsTab::Activity
            && self.activity_auto_refresh
            && self.pending_activity.is_none()
            && self
                .activity_last_refresh
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            self.refresh_activity();
        }
        if self.mode == ViewMode::Activity
            && self.operations_tab == OperationsTab::Jobs
            && self.pending_jobs.is_none()
            && self
                .operations_jobs_last_refresh
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
        {
            self.reload_jobs();
        }
        let project_options = self.project_catalog.clone();
        let project_catalog_error = self.project_catalog_error.clone();

        let http_available = self.activity_base().is_some();
        let wiki_available = self.available_load_source().is_some();
        let rail_output = draw_rail(root_ui, self.shell_route(), http_available, wiki_available);
        if let Some(route) = rail_output.navigate {
            self.activate_shell_route(route);
        }

        let project_switch_enabled = !self.wiki_edit.as_ref().is_some_and(|edit| edit.dirty)
            && !self.noncancellable_mutation_in_flight();
        let connected = http_available;
        let read_only_source = self.shell_read_only_source();
        let (healthy, health_summary) = self.shell_health();
        let route = self.shell_route();
        let mutation_label = self.mutation_status_label();
        let topbar_output = draw_topbar(
            root_ui,
            TopbarState {
                route,
                project: &mut self.filter_wing,
                projects: &project_options,
                project_enabled: project_switch_enabled,
                project_loading: self.pending_project_catalog.is_some(),
                project_error: project_catalog_error.as_deref(),
                search_enabled: connected,
                connected,
                read_only_source,
                healthy,
                health_summary: &health_summary,
                mutation_label,
            },
        );
        if topbar_output.retry_projects {
            self.reload_project_catalog();
        }
        if topbar_output.open_search {
            self.activate_shell_route(ShellRoute::Search);
            if self.mode == ViewMode::Search {
                ctx.memory_mut(|memory| {
                    memory.request_focus(egui::Id::new("native_search_query"));
                });
            }
        }

        // Only workspaces with real contextual controls receive a second tier.
        // Summary-only rows on Home/Search/Library duplicate page content and
        // waste vertical space, especially on compact native windows.
        if matches!(
            self.mode,
            ViewMode::Activity | ViewMode::Wiki | ViewMode::Graph
        ) {
            egui::Panel::top("workspace_toolbar")
                .resizable(false)
                .frame(theme::toolbar_frame())
                .show(root_ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                match self.mode {
                    ViewMode::Home => {
                        if self.pending_project_home.is_some() {
                            ui.spinner();
                            ui.weak("инвентаризация…");
                        } else if let Some(home) = &self.project_home {
                            ui.weak(format!("{} документов", home.documents));
                        }
                    }
                    ViewMode::Library => {
                        if self.pending_library.is_some() {
                            ui.spinner();
                            ui.weak("каталог…");
                        } else if let Some(page) = &self.library_page {
                            ui.weak(format!("{} документов", page.total));
                        }
                    }
                    ViewMode::Search => {
                        if self.pending_search.is_some() {
                            ui.spinner();
                            ui.weak("поиск…");
                        } else if let Some(results) = &self.search_results {
                            ui.weak(format!("{} результатов", results.items.len()));
                        }
                    }
                    ViewMode::Revisions => {
                        if self.pending_revisions.is_some()
                            || self.pending_revision_snapshot.is_some()
                            || self.pending_revision_diff.is_some()
                            || self.pending_revision_restore.is_some()
                        {
                            ui.spinner();
                            ui.weak("история…");
                        } else if let Some(head) = self.revision.head {
                            ui.weak(format!("текущая r{head}"));
                        }
                    }
                    ViewMode::Activity => {
                        if ui
                            .selectable_label(
                                self.operations_tab == OperationsTab::Activity,
                                "Активность",
                            )
                            .clicked()
                        {
                            self.operations_tab = OperationsTab::Activity;
                            self.refresh_activity();
                        }
                        if ui
                            .selectable_label(self.operations_tab == OperationsTab::Jobs, "Задачи")
                            .clicked()
                        {
                            self.operations_tab = OperationsTab::Jobs;
                            self.reload_jobs();
                        }
                        if ui
                            .selectable_label(
                                self.operations_tab == OperationsTab::Maintenance,
                                "Диагностика и бэкап",
                            )
                            .clicked()
                        {
                            self.operations_tab = OperationsTab::Maintenance;
                            if self.operations_snapshot.is_none() {
                                self.reload_operations();
                            }
                        }
                        ui.separator();
                        if self.operations_tab == OperationsTab::Activity {
                            if ui.button("Обновить").clicked() {
                                self.refresh_activity();
                            }
                            ui.checkbox(&mut self.activity_auto_refresh, "Онлайн");
                        } else if self.operations_tab == OperationsTab::Jobs {
                            if self.pending_jobs.is_some()
                                || self.pending_operation_action.is_some()
                            {
                                ui.spinner();
                            }
                            ui.weak(format!("{} сохранено", self.operations_jobs.len()));
                        } else {
                            if self.pending_operations.is_some()
                                || self.pending_operation_action.is_some()
                            {
                                ui.spinner();
                            }
                            if self.operations_snapshot_stale {
                                ui.colored_label(theme::WARN, "устарело");
                            } else if let Some(snapshot) = &self.operations_snapshot {
                                ui.colored_label(
                                    if snapshot.doctor.ok {
                                        egui::Color32::from_rgb(90, 190, 125)
                                    } else {
                                        egui::Color32::from_rgb(220, 105, 90)
                                    },
                                    if snapshot.doctor.ok {
                                        "в норме"
                                    } else {
                                        "требует внимания"
                                    },
                                );
                            }
                        }
                        if self.operations_tab != OperationsTab::Activity {
                            return;
                        }
                        let filters_active = self.activity_kind_filter != "all"
                            || self.activity_status_filter != "all"
                            || !self.activity_client_filter.is_empty()
                            || !self.activity_action_filter.is_empty()
                            || !self.activity_filter.is_empty();
                        ui.menu_button(
                            if filters_active {
                                "Фильтры (активны)"
                            } else {
                                "Фильтры"
                            },
                            |ui| {
                                ui.set_min_width(260.0);
                                ui.label("Тип события");
                                egui::ComboBox::from_id_salt("activity_kind_filter")
                                    .selected_text(match self.activity_kind_filter.as_str() {
                                        "http" => "HTTP",
                                        "mcp_tool" => "MCP tool",
                                        _ => "Все типы",
                                    })
                                    .show_ui(ui, |ui| {
                                        for (value, label) in [
                                            ("all", "Все типы"),
                                            ("http", "HTTP"),
                                            ("mcp_tool", "MCP tool"),
                                        ] {
                                            crate::ui::closing_selectable_value(
                                                ui,
                                                &mut self.activity_kind_filter,
                                                value.to_string(),
                                                label,
                                            );
                                        }
                                    });
                                ui.label("Результат");
                                egui::ComboBox::from_id_salt("activity_status_filter")
                                    .selected_text(match self.activity_status_filter.as_str() {
                                        "success" => "Успех",
                                        "error" => "Ошибки",
                                        _ => "Все результаты",
                                    })
                                    .show_ui(ui, |ui| {
                                        for (value, label) in [
                                            ("all", "Все результаты"),
                                            ("success", "Успех"),
                                            ("error", "Ошибки"),
                                        ] {
                                            crate::ui::closing_selectable_value(
                                                ui,
                                                &mut self.activity_status_filter,
                                                value.to_string(),
                                                label,
                                            );
                                        }
                                    });
                                ui.label("Клиент");
                                ui.text_edit_singleline(&mut self.activity_client_filter);
                                ui.label("Действие или маршрут");
                                ui.text_edit_singleline(&mut self.activity_action_filter);
                                ui.label("Любое поле");
                                ui.text_edit_singleline(&mut self.activity_filter);
                                if ui
                                    .add_enabled(filters_active, egui::Button::new("Сбросить фильтры"))
                                    .clicked()
                                {
                                    self.activity_kind_filter = "all".into();
                                    self.activity_status_filter = "all".into();
                                    self.activity_client_filter.clear();
                                    self.activity_action_filter.clear();
                                    self.activity_filter.clear();
                                }
                            },
                        );
                        if self.pending_activity.is_some() {
                            ui.spinner();
                        }
                    }
                    ViewMode::Evaluation => {
                        ui.label(egui::RichText::new("EVAL").monospace().color(theme::L1));
                        ui.weak("живая telemetry · benchmark-значения не угадываются");
                        if self.pending_activity.is_some() {
                            ui.spinner();
                        } else if ui.button("Обновить telemetry").clicked() {
                            self.refresh_activity();
                        }
                    }
                    ViewMode::Models => {
                        ui.label(
                            egui::RichText::new("PIPELINE")
                                .monospace()
                                .color(theme::L3),
                        );
                        if self.pending_operations.is_some() {
                            ui.spinner();
                        } else if let Some(snapshot) = &self.operations_snapshot {
                            ui.weak(format!(
                                "{} · схема v{} · {} чанков",
                                snapshot.status.backend,
                                snapshot.status.schema_version,
                                snapshot.status.chunk_count
                            ));
                        }
                    }
                    ViewMode::Sync => {
                        ui.label(egui::RichText::new("SYNC").monospace().color(theme::L2));
                        if let Some(sync) = self.operations_snapshot.as_ref().and_then(|snapshot| snapshot.sync.as_ref()) {
                            ui.weak(format!("{} · cursor {} · outbox {}", sync.role, sync.latest_primary_seq, sync.pending_outbox));
                        } else { ui.weak("ожидание sync status"); }
                    }
                    ViewMode::Wiki => {
                        if ui
                            .selectable_label(self.wiki_sidebar_visible, "Страницы")
                            .on_hover_text("Показать или скрыть каталог страниц")
                            .clicked()
                        {
                            self.wiki_sidebar_visible = !self.wiki_sidebar_visible;
                        }
                        if ui
                            .add_enabled(!self.wiki_history.is_empty(), egui::Button::new("< Назад"))
                            .on_hover_text("Назад по истории страниц (Alt+Left)")
                            .clicked()
                        {
                            self.wiki_go_back();
                        }
                        if ui.button("Обновить вики").clicked() {
                            if self.pending_save.is_some() {
                                self.wiki_error = Some(
                                    "Идёт сохранение. Дождитесь результата перед обновлением вики."
                                        .into(),
                                );
                            } else if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
                                self.wiki_error = Some(
                                    "Есть несохранённые правки: сохраните или отмените их".into(),
                                );
                            } else {
                                self.wiki_edit = None;
                                self.wiki_loaded = false;
                                self.reload_wiki_catalog();
                            }
                        }
                        if self.pending_catalog.is_some() {
                            ui.spinner();
                            ui.weak("каталог…");
                        }
                        if self.pending_page_a.is_some() || self.pending_page_b.is_some() {
                            ui.spinner();
                            ui.weak("страница…");
                        }
                        if ui
                            .selectable_label(self.wiki_dual_pane, "Две панели")
                            .on_hover_text(
                                "Две статьи: каталог слева, панель A по центру, B справа",
                            )
                            .clicked()
                        {
                            self.wiki_dual_pane = !self.wiki_dual_pane;
                            if self.wiki_dual_pane {
                                // Next sidebar click fills the empty secondary column.
                                self.wiki_focus = WikiPane::B;
                            } else {
                                self.clear_wiki_pane_b();
                                self.wiki_focus = WikiPane::A;
                            }
                        }
                        if !self.wiki_dual_pane
                            && ui
                                .selectable_label(self.wiki_info_visible, "Инфо")
                                .on_hover_text("Показать метаданные страницы и обратные ссылки")
                                .clicked()
                        {
                            self.wiki_info_visible = !self.wiki_info_visible;
                        }
                        if self.wiki_dual_pane {
                            if ui
                                .selectable_label(self.wiki_focus == WikiPane::A, "A")
                                .on_hover_text("Фокус панели A; редактирование и сохранение работают здесь")
                                .clicked()
                            {
                                self.wiki_focus = WikiPane::A;
                            }
                            if ui
                                .selectable_label(self.wiki_focus == WikiPane::B, "B")
                                .on_hover_text("Фокус панели B; каталог открывает страницы здесь")
                                .clicked()
                            {
                                self.wiki_focus = WikiPane::B;
                            }
                        }
                        let editing = self.wiki_edit.is_some();
                        if !editing {
                            if ui
                                .add_enabled(
                                    self.wiki_article.is_some() && self.wiki_can_write(),
                                    egui::Button::new("Редактировать"),
                                )
                                .on_hover_text("Редактировать панель A; сохранение идёт через HTTP gateway")
                                .clicked()
                            {
                                self.wiki_focus = WikiPane::A;
                                self.start_wiki_edit();
                            }
                        } else {
                            let saving = self.pending_save.is_some();
                            if ui
                                .add_enabled(
                                    self.wiki_can_write() && !saving,
                                    egui::Button::new("Сохранить"),
                                )
                                .on_hover_text("Ctrl+S")
                                .clicked()
                            {
                                self.save_wiki_edit();
                            }
                            if saving {
                                ui.spinner();
                                ui.weak("сохранение…");
                            }
                            if ui
                                .add_enabled(can_cancel_edit(saving), egui::Button::new("Отменить правки"))
                                .on_hover_text(if saving {
                                    "Дождитесь результата сохранения"
                                } else {
                                    "Esc"
                                })
                                .clicked()
                            {
                                self.cancel_wiki_edit();
                            }
                        }
                        if ui
                            .add_enabled(
                                self.wiki_selected_id.is_some() && !editing,
                                egui::Button::new("Показать в графе"),
                            )
                            .clicked()
                        {
                            if let Some((id, title)) = self
                                .wiki_article
                                .as_ref()
                                .map(|a| (a.id.clone(), a.title.clone()))
                            {
                                self.open_graph_focus(id, Some(title));
                            }
                        }
                    }
                    ViewMode::Graph => {
                        let content_available = self.available_load_source().is_some();
                        let previous_lens = self.graph_lens;
                        let lens_response = egui::ComboBox::from_id_salt("graph_lens")
                            .selected_text(self.graph_lens.label())
                            .width(130.0)
                            .show_ui(ui, |ui| {
                                for lens in GraphLens::ALL {
                                    ui.selectable_value(
                                        &mut self.graph_lens,
                                        lens,
                                        lens.label(),
                                    );
                                }
                            });
                        lens_response.response.on_hover_text(self.graph_lens.description());
                        if self.graph_lens != previous_lens {
                            self.selected = None;
                            self.content = None;
                            self.rebuild_ui_graph();
                        }
                        ui.weak("Фокус");
                        let seed_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.seed_input)
                                .desired_width(180.0)
                                .hint_text("Найти узел…"),
                        );
                        // Seed picker: suggestions over full_view (label / id /
                        // document_id substring, case-insensitive, first 10).
                        let q = self.seed_input.trim().to_lowercase();
                        let mut suggestions: Vec<(String, String)> = Vec::new();
                        if seed_resp.has_focus() && !q.is_empty() {
                            if let Some(full) = self.full_view.as_ref() {
                                for n in &full.nodes {
                                    let doc = n.document_id.as_deref().unwrap_or("");
                                    if n.label.to_lowercase().contains(&q)
                                        || n.id.to_lowercase().contains(&q)
                                        || doc.to_lowercase().contains(&q)
                                    {
                                        suggestions.push((n.id.clone(), n.label.clone()));
                                        if suggestions.len() >= 10 {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
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
                                self.full_view.as_ref(),
                                &self.seed_input,
                                first_pick.as_deref(),
                            ) {
                                self.seed_input = id;
                            }
                            self.submit_graph_focus();
                        } else if let Some(id) = picked {
                            seed_resp.surrender_focus();
                            self.seed_input = id;
                            self.submit_graph_focus();
                        }
                        if ui.button("Показать").clicked() {
                            self.submit_graph_focus();
                        }
                        ui.menu_button("Вид", |ui| {
                            ui.set_min_width(220.0);
                            ui.horizontal(|ui| {
                                ui.label("Глубина связей");
                                let mut d = self.depth as i32;
                                if ui.add(egui::DragValue::new(&mut d).range(1..=3)).changed() {
                                    self.depth = d as u32;
                                    if self.topology_query_requires_reload() {
                                        self.dispatch_graph_load();
                                    } else {
                                        self.rebuild_ui_graph();
                                    }
                                }
                            });
                            ui.checkbox(&mut self.show_tags, "Показывать теги");
                            ui.checkbox(&mut self.show_stubs, "Показывать заглушки");
                            ui.separator();
                            ui.label("Комната");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.filter_room)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("Все комнаты"),
                            );
                            if ui
                                .add_enabled(
                                    !self.filter_room.is_empty(),
                                    egui::Button::new("Сбросить фильтр комнаты"),
                                )
                                .clicked()
                            {
                                self.filter_room.clear();
                            }
                        });
                        if ui
                            .button("Сбросить вид")
                            .on_hover_text(
                                "Перестроить локальный вид от фокуса и убрать добавленные раскрытия",
                            )
                            .clicked()
                        {
                            if self.expanded_dirty {
                                self.confirm_reset = true;
                            } else {
                                self.rebuild_ui_graph();
                            }
                        }
                        let expanding = self.pending_expand.is_some();
                        if ui
                            .add_enabled(
                                self.selected.is_some() && !expanding,
                                egui::Button::new("Раскрыть"),
                            )
                            .clicked()
                        {
                            self.expand_selected();
                        }
                        if expanding {
                            ui.spinner();
                        }
                        let open_page = ui
                            .add_enabled(
                                content_available
                                    && self.selected_graph_wiki_target().is_some(),
                                egui::Button::new("Открыть страницу"),
                            )
                            .on_hover_text(if content_available {
                                "Открыть выбранную wiki-страницу"
                            } else {
                                "Snapshot содержит только топологию; содержимое требует HTTP или DB"
                            });
                        if open_page.clicked() {
                            self.open_selected_graph_node_in_wiki();
                        }
                        let preview = ui
                            .add_enabled(
                                content_available
                                    && self.selected.is_some()
                                    && self.pending_content.is_none(),
                                egui::Button::new("Превью"),
                            )
                            .on_hover_text(if content_available {
                                "Загрузить содержимое выбранного документа"
                            } else {
                                "Snapshot содержит только топологию; содержимое требует HTTP или DB"
                            });
                        if preview.clicked() {
                            self.load_content_for_selected();
                        }
                        if self.pending_content.is_some() {
                            ui.spinner();
                        }
                    }
                }
                });
            });
        }

        let mutation_in_flight = self.noncancellable_mutation_in_flight();
        let project_changed = reconcile_project_change(
            &mut self.filter_wing,
            &self.prev_filter_wing,
            mutation_in_flight,
        );
        let tags_changed = self.show_tags != self.prev_show_tags;
        if tags_changed
            || self.show_stubs != self.prev_show_stubs
            || project_changed
            || self.filter_room != self.prev_filter_room
        {
            self.prev_show_tags = self.show_tags;
            self.prev_show_stubs = self.show_stubs;
            self.prev_filter_wing = self.filter_wing.clone();
            self.prev_filter_room = self.filter_room.clone();
            if project_changed {
                self.reset_product_for_project();
                self.reset_wiki_for_project();
                // A seed belongs to the previous project. Loading the new
                // project through /neighbors with that stale seed would 404
                // and leave the old graph visible.
                self.seed_input.clear();
                self.seed_id = None;
                self.seed_error = None;
                self.selected = None;
                self.content = None;
                self.content_error = None;
            }
            let project_needs_reload = project_changed && self.topology_query_requires_reload();
            let tags_need_reload = tags_changed && self.topology_query_requires_reload();
            if project_needs_reload || tags_need_reload {
                self.dispatch_graph_load();
            } else {
                self.rebuild_ui_graph();
            }
        }

        egui::Panel::bottom("status")
            .resizable(false)
            .frame(theme::toolbar_frame())
            .show(root_ui, |ui| match self.mode {
                ViewMode::Home => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Пульт проекта");
                        ui.separator();
                        if let Some(project) = nonempty(&self.filter_wing) {
                            ui.label(project);
                        } else {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 150, 65),
                                "выберите проект",
                            );
                        }
                        if self.pending_project_home.is_some() {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Обновление…");
                        }
                    });
                }
                ViewMode::Library => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Корпус");
                        if let Some(page) = &self.library_page {
                            ui.separator();
                            ui.label(format!("{} всего", page.total));
                            ui.label(format!("{} на странице", page.items.len()));
                        }
                        if self.pending_library.is_some() || self.pending_library_document.is_some()
                        {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Обновление…");
                        }
                    });
                }
                ViewMode::Search => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Поиск");
                        ui.separator();
                        ui.label(match self.search_request.mode.as_str() {
                            "lex" => "лексический",
                            "vec" => "семантический",
                            _ => "hybrid",
                        });
                        if let Some(project) = self.search_request.wing.as_deref() {
                            ui.separator();
                            ui.label(project);
                        }
                        if self.pending_search.is_some() {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Поиск…");
                        }
                    });
                }
                ViewMode::Revisions => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("История версий");
                        if let Some(head) = self.revision.head {
                            ui.separator();
                            ui.label(format!("текущая r{head}"));
                        }
                        if let Some(selected) = self.revision.selected {
                            ui.separator();
                            ui.label(format!("сравнение с r{selected}"));
                        }
                        if self.pending_revisions.is_some()
                            || self.pending_revision_snapshot.is_some()
                            || self.pending_revision_diff.is_some()
                            || self.pending_revision_restore.is_some()
                        {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Обновление…");
                        }
                    });
                }
                ViewMode::Activity => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Операции");
                        ui.separator();
                        match self.operations_tab {
                            OperationsTab::Activity => {
                                ui.label(format!("{} событий сохранено", self.activity.len()));
                                ui.weak("тела запросов и секретные заголовки не записываются");
                            }
                            OperationsTab::Jobs => {
                                ui.label(format!("{} задач сохранено", self.operations_jobs.len()));
                                ui.weak("единая очередь записи");
                            }
                            OperationsTab::Maintenance => {
                                let healthy = self
                                    .operations_snapshot
                                    .as_ref()
                                    .is_some_and(|snapshot| snapshot.doctor.ok)
                                    && !self.operations_snapshot_stale;
                                ui.colored_label(
                                    if healthy {
                                        egui::Color32::from_rgb(90, 190, 125)
                                    } else {
                                        egui::Color32::from_rgb(220, 150, 65)
                                    },
                                    if self.operations_snapshot_stale {
                                        "состояние устарело"
                                    } else if healthy {
                                        "в норме"
                                    } else {
                                        "проверьте диагностику"
                                    },
                                );
                            }
                        }
                    });
                }
                ViewMode::Evaluation => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Оценка");
                        ui.separator();
                        ui.label(format!("{} событий telemetry", self.activity.len()));
                        ui.weak("отсутствующие benchmark-метрики показаны как —");
                    });
                }
                ViewMode::Sync => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Синхронизация БД");
                        if let Some(sync) = self
                            .operations_snapshot
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
                ViewMode::Models => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Runtime и индекс");
                        if let Some(snapshot) = &self.operations_snapshot {
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
                ViewMode::Wiki => {
                    ui.horizontal_wrapped(|ui| {
                        let (source_color, source_status) = if self.activity_base().is_some() {
                            (theme::OK, "Gateway")
                        } else {
                            match self.source.as_ref() {
                                Some(GraphSourceKind::LiveStore { .. }) => {
                                    (theme::ACCENT, "DB · только чтение")
                                }
                                Some(
                                    GraphSourceKind::SnapshotFile { .. }
                                    | GraphSourceKind::VaultGraphJson { .. },
                                ) => (theme::FAINT, "Snapshot · только чтение"),
                                Some(GraphSourceKind::HttpService { .. }) => {
                                    (theme::WARN, "Gateway · неизвестно")
                                }
                                None => (theme::DANGER, "Офлайн"),
                            }
                        };
                        ui.colored_label(source_color, source_status).on_hover_text(
                            self.source
                                .as_ref()
                                .map(|source| source.label())
                                .unwrap_or("Источник данных не выбран"),
                        );
                        ui.separator();
                        ui.label(format!("{} страниц", self.wiki_pages.len()));
                        if self.any_pending() {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Обновление…");
                        }
                        if self.wiki_dual_pane {
                            ui.separator();
                            ui.label("Две панели");
                        }
                        if self.wiki_edit.is_some() {
                            ui.separator();
                            ui.strong("Редактирование");
                            if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
                                ui.colored_label(
                                    egui::Color32::from_rgb(220, 160, 60),
                                    "не сохранено",
                                );
                            }
                        }
                        if let Some(note) = &self.wiki_save_note {
                            ui.separator();
                            ui.colored_label(egui::Color32::from_rgb(100, 180, 120), note);
                        }
                    });
                }
                ViewMode::Graph => {
                    let seed_label = self.seed_id.as_deref().or(if self.seed_input.is_empty() {
                        None
                    } else {
                        Some(self.seed_input.as_str())
                    });
                    let truncated = self
                        .ui_graph
                        .as_ref()
                        .is_some_and(|g| g.truncated_nodes || g.truncated_edges)
                        || self.raw_truncated
                        || self.local_truncated;
                    draw_status(
                        ui,
                        self.source.as_ref(),
                        seed_label,
                        self.depth,
                        self.ui_graph.as_ref(),
                        self.layout_ready,
                        truncated,
                        self.status_banner().as_deref(),
                        self.any_pending(),
                    );
                }
            });

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
                        nonempty(&self.filter_wing).as_deref(),
                        self.project_home.as_ref(),
                        self.operations_snapshot.as_ref(),
                        &self.activity,
                        self.project_home_error.as_deref(),
                        self.operations_error.as_deref(),
                        self.operations_snapshot_stale,
                        self.pending_project_home.is_some()
                            || self.pending_operations.is_some()
                            || self.pending_activity.is_some(),
                        self.activity_base().is_some(),
                        self.available_load_source().is_some(),
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
                            let sel = if self.wiki_dual_pane && self.wiki_focus == WikiPane::B {
                                self.wiki_selected_id_b.as_deref()
                            } else {
                                self.wiki_selected_id.as_deref()
                            };
                            if let Some(id) = draw_wiki_sidebar(
                                ui,
                                &self.wiki_pages,
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
                            let (titles, slugs) = self.wiki_known_keys();
                            let action = draw_wiki_read_view(
                                ui,
                                self.wiki_article_b.as_ref(),
                                self.wiki_error_b.as_deref(),
                                WikiReadContext {
                                    known_titles: &titles,
                                    known_slugs: &slugs,
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
                        let (titles, slugs) = self.wiki_known_keys();
                        let action = draw_wiki_read_view(
                            ui,
                            self.wiki_article.as_ref(),
                            self.wiki_error.as_deref(),
                            WikiReadContext {
                                known_titles: &titles,
                                known_slugs: &slugs,
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

        // "Reset to seed" confirmation (only when Expand merges would be lost).
        if self.confirm_reset {
            let mut do_reset = false;
            let mut stay = true;
            egui::Window::new("Сбросить к фокусу")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label("Убрать узлы, добавленные раскрытием соседей, и перестроить граф от фокуса?");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Сбросить").clicked() {
                            do_reset = true;
                            stay = false;
                        }
                        if ui.button("Отмена").clicked() {
                            stay = false;
                        }
                    });
                });
            if !stay {
                self.confirm_reset = false;
            }
            if do_reset {
                self.rebuild_ui_graph();
            }
        }

        self.draw_close_confirmation(&ctx);

        // Keep spinner animations / pending states alive while the worker runs.
        if self.any_pending() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }
}
