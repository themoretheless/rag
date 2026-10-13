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
                        // Cached per (graph revision, query): the scan reruns
                        // only when either changes.
                        let q = self.seed_input.trim().to_lowercase();
                        let suggestions: Vec<(String, String)> =
                            if seed_resp.has_focus() && !q.is_empty() {
                                self.seed_suggestions(&q)
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
    }

    pub(super) fn draw_status_bar(&mut self, root_ui: &mut egui::Ui) {
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
    }
}
