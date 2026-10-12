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
        self.draw_workspace_toolbar(root_ui);

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

        self.draw_status_bar(root_ui);

        self.draw_workspace(root_ui, &ctx);

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
