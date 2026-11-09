//! Per-frame layout: shell, workspaces, panels and dialogs.

use super::*;

impl eframe::App for GraphApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.drain_worker_events();
        self.intercept_native_close(&ctx);
        self.handle_hotkeys(&ctx);
        if self.mode == ViewMode::Activity
            && self.ops.tab == OperationsTab::Activity
            && self.activity.auto_refresh
            && self.pending.activity.is_none()
            && self
                .activity
                .last_refresh
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            self.refresh_activity();
        }
        if self.mode == ViewMode::Activity
            && self.ops.tab == OperationsTab::Jobs
            && self.pending.jobs.is_none()
            && self
                .ops
                .jobs_last_refresh
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
        {
            self.reload_jobs();
        }
        let project_options = self.scope.catalog.clone();
        let project_catalog_error = self.scope.catalog_error.clone();

        let http_available = self.activity_base().is_some();
        let wiki_available = self.available_load_source().is_some();
        let rail_output = draw_rail(root_ui, self.shell_route(), http_available, wiki_available);
        if let Some(route) = rail_output.navigate {
            self.activate_shell_route(route);
        }

        let project_switch_enabled = !self.wiki.edit.as_ref().is_some_and(|edit| edit.dirty)
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
                project: &mut self.scope.wing,
                projects: &project_options,
                project_enabled: project_switch_enabled,
                project_loading: self.pending.project_catalog.is_some(),
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
            &mut self.scope.wing,
            &self.scope.prev_wing,
            mutation_in_flight,
        );
        let tags_changed = self.graph.show_tags != self.graph.prev_show_tags;
        if tags_changed
            || self.graph.show_stubs != self.graph.prev_show_stubs
            || project_changed
            || self.scope.room != self.scope.prev_room
        {
            self.graph.prev_show_tags = self.graph.show_tags;
            self.graph.prev_show_stubs = self.graph.show_stubs;
            self.scope.prev_wing = self.scope.wing.clone();
            self.scope.prev_room = self.scope.room.clone();
            if project_changed {
                self.reset_product_for_project();
                self.reset_wiki_for_project();
                // A seed belongs to the previous project. Loading the new
                // project through /neighbors with that stale seed would 404
                // and leave the old graph visible.
                self.graph.seed_input.clear();
                self.graph.seed_id = None;
                self.graph.seed_error = None;
                self.graph.selected = None;
                self.graph.content = None;
                self.graph.content_error = None;
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
        if self.graph.confirm_reset {
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
                self.graph.confirm_reset = false;
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
