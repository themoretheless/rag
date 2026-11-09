//! Applies worker events to app state (seq-checked).

use super::*;

impl GraphApp {
    /// Drain worker results; stale seqs are ignored (a newer job superseded them).
    pub(super) fn drain_worker_events(&mut self) {
        while let Ok(evt) = self.worker.rx.try_recv() {
            match evt {
                WorkerEvt::GraphLoaded { seq, result } => {
                    if !self.pending.graph.accept(seq) {
                        continue;
                    }
                    match result {
                        Ok(loaded) => self.apply_loaded(loaded),
                        Err(e) => {
                            self.load_error = Some(e);
                            if self.mode == ViewMode::Wiki {
                                // Wiki can still answer (or fail with a clearer error).
                                self.ensure_wiki_loaded();
                            }
                        }
                    }
                }
                WorkerEvt::ProjectCatalogLoaded { seq, result } => {
                    if !self.pending.project_catalog.accept(seq) {
                        continue;
                    }
                    match result {
                        Ok(projects) => {
                            self.project_catalog = projects;
                            self.project_catalog_error = None;
                        }
                        Err(error) => self.project_catalog_error = Some(error),
                    }
                }
                WorkerEvt::ProjectHomeLoaded {
                    seq,
                    project,
                    result,
                } => {
                    if !self.pending.project_home.accept(seq) {
                        continue;
                    }
                    if project != self.filter_wing.trim() {
                        continue;
                    }
                    self.project_home_project = Some(project);
                    match result {
                        Ok(home) => {
                            self.project_home = Some(home);
                            self.project_home_error = None;
                        }
                        Err(error) => {
                            self.project_home = None;
                            self.project_home_error = Some(error);
                        }
                    }
                }
                WorkerEvt::LibraryLoaded {
                    seq,
                    project,
                    result,
                } => {
                    if !self.pending.library.accept(seq) {
                        continue;
                    }
                    if !project_scope_matches(project.as_deref(), &self.filter_wing) {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            let selected_still_visible =
                                self.library_selected_id.as_ref().is_some_and(|selected| {
                                    page.items.iter().any(|item| &item.id == selected)
                                });
                            if !selected_still_visible {
                                self.library_selected_id = None;
                                self.library_document = None;
                                self.library_document_error = None;
                                self.pending.library_document.clear();
                            }
                            self.library_page = Some(page);
                            self.library_error = None;
                            if let Some(id) = self.library_open_after_load.take() {
                                if self
                                    .library_page
                                    .as_ref()
                                    .is_some_and(|page| page.items.iter().any(|item| item.id == id))
                                {
                                    self.select_library_document(&id);
                                } else {
                                    self.library_error = Some(format!(
                                        "Документ {id} не найден текущим фильтром каталога"
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            self.library_page = None;
                            self.library_error = Some(error);
                        }
                    }
                }
                WorkerEvt::LibraryDocumentLoaded {
                    seq,
                    document_id,
                    result,
                } => {
                    if !self.pending.library_document.accept(seq) {
                        continue;
                    }
                    if self.library_selected_id.as_deref() != Some(document_id.as_str()) {
                        continue;
                    }
                    match result {
                        Ok(body) => {
                            self.library_document = Some(body);
                            self.library_document_error = None;
                        }
                        Err(error) => {
                            self.library_document = None;
                            self.library_document_error = Some(error);
                        }
                    }
                }
                WorkerEvt::SearchLoaded {
                    seq,
                    project,
                    result,
                } => {
                    if !self.pending.search.accept(seq) {
                        continue;
                    }
                    if !project_scope_matches(project.as_deref(), &self.filter_wing) {
                        continue;
                    }
                    match result {
                        Ok(results) => {
                            self.search_results = Some(results);
                            self.search_error = None;
                        }
                        Err(error) => {
                            self.search_results = None;
                            self.search_error = Some(error);
                        }
                    }
                }
                WorkerEvt::OperationsLoaded { seq, result } => {
                    if !self.pending.operations.accept(seq) {
                        continue;
                    }
                    match result {
                        Ok(snapshot) => {
                            self.operations_snapshot = Some(snapshot);
                            self.operations_snapshot_stale = false;
                            self.operations_error = None;
                        }
                        Err(error) => {
                            self.operations_snapshot_stale = true;
                            self.operations_error = Some(error);
                        }
                    }
                }
                WorkerEvt::JobsLoaded { seq, result } => {
                    if !self.pending.jobs.accept(seq) {
                        continue;
                    }
                    self.operations_jobs_last_refresh = Some(Instant::now());
                    match result {
                        Ok(jobs) => {
                            self.operations_jobs = jobs;
                            self.operations_error = None;
                        }
                        Err(error) => self.operations_error = Some(error),
                    }
                }
                WorkerEvt::JobChanged { seq, result } => {
                    if !self.pending.operation_action.accept(seq) {
                        continue;
                    }
                    match *result {
                        Ok(job) => {
                            if let Some(existing) = self
                                .operations_jobs
                                .iter_mut()
                                .find(|existing| existing.id == job.id)
                            {
                                *existing = job;
                            } else {
                                self.operations_jobs.insert(0, job);
                            }
                            self.operations_error = None;
                            self.reload_jobs();
                        }
                        Err(error) => self.operations_error = Some(error),
                    }
                }
                WorkerEvt::MaintenanceCompleted { seq, result } => {
                    if !self.pending.operation_action.accept(seq) {
                        continue;
                    }
                    match result {
                        Ok(result) => {
                            self.operations_last_result = Some(result);
                            self.operations_error = None;
                            self.reload_operations();
                        }
                        Err(error) => self.operations_error = Some(error),
                    }
                }
                WorkerEvt::RevisionsLoaded {
                    seq,
                    document_id,
                    append,
                    result,
                } => {
                    if !self.pending.revisions.accept(seq) {
                        continue;
                    }
                    if self.revision.document_id.as_deref() != Some(document_id.as_str()) {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            if append {
                                for item in page.items {
                                    if !self
                                        .revision
                                        .items
                                        .iter()
                                        .any(|existing| existing.revision == item.revision)
                                    {
                                        self.revision.items.push(item);
                                    }
                                }
                            } else {
                                self.revision.items = page.items;
                            }
                            self.revision.total = page.total;
                            self.revision.next_cursor = page.next_cursor;
                            self.revision.error = None;
                        }
                        Err(error) => self.revision.error = Some(error),
                    }
                }
                WorkerEvt::RevisionSnapshotLoaded {
                    seq,
                    document_id,
                    revision,
                    result,
                } => {
                    if !self.pending.revision_snapshot.accept(seq) {
                        continue;
                    }
                    if self.revision.document_id.as_deref() != Some(document_id.as_str())
                        || self.revision.selected != Some(revision)
                    {
                        continue;
                    }
                    match result {
                        Ok(snapshot) => {
                            self.revision.snapshot = Some(snapshot);
                            self.revision.snapshot_error = None;
                        }
                        Err(error) => self.revision.snapshot_error = Some(error),
                    }
                }
                WorkerEvt::RevisionDiffLoaded {
                    seq,
                    document_id,
                    from_revision,
                    result,
                } => {
                    if !self.pending.revision_diff.accept(seq) {
                        continue;
                    }
                    if self.revision.document_id.as_deref() != Some(document_id.as_str())
                        || self.revision.selected != Some(from_revision)
                    {
                        continue;
                    }
                    match result {
                        Ok(diff) => {
                            self.revision.diff = Some(diff);
                            self.revision.diff_error = None;
                        }
                        Err(error) => self.revision.diff_error = Some(error),
                    }
                }
                WorkerEvt::RevisionRestored {
                    seq,
                    document_id,
                    result,
                } => {
                    if !self.pending.revision_restore.accept(seq) {
                        continue;
                    }
                    if self.revision.document_id.as_deref() != Some(document_id.as_str()) {
                        continue;
                    }
                    match result {
                        Ok(result) => {
                            self.revision.head = Some(result.revision);
                            self.revision.restore_result = Some(result);
                            self.revision.restore_confirm = None;
                            self.revision.diff = None;
                            self.revision.diff_error = None;
                            self.revision.snapshot = None;
                            self.revision.snapshot_error = None;
                            self.revision.selected = None;
                            self.revision.error = None;
                            self.reload_revisions();
                            if self.library_selected_id.as_deref()
                                == self.revision.document_id.as_deref()
                            {
                                let id = document_id.clone();
                                self.select_library_document(&id);
                            }
                        }
                        Err(error) => self.revision.error = Some(error),
                    }
                }
                WorkerEvt::WikiCatalog { seq, result } => {
                    if !self.pending.catalog.accept(seq) {
                        continue;
                    }
                    self.wiki_loaded = true;
                    match result {
                        Ok(mut pages) => {
                            sort_wiki_pages(&mut pages);
                            self.wiki_pages = pages;
                            self.wiki_pages_rev += 1;
                            if let Some(id) = self.wiki_open_after_catalog.take() {
                                if self.wiki_pages.iter().any(|page| page.id == id) {
                                    self.open_wiki_page_id(&id);
                                } else {
                                    self.wiki_error =
                                        Some(format!("Документа {id} нет в wiki-каталоге проекта"));
                                }
                            } else {
                                self.auto_open_initial_page();
                            }
                        }
                        Err(e) => self.wiki_error = Some(e),
                    }
                }
                WorkerEvt::PageOpened {
                    seq,
                    pane_b,
                    push_history,
                    q,
                    result,
                } => {
                    let slot = if pane_b {
                        &mut self.pending.page_b
                    } else {
                        &mut self.pending.page_a
                    };
                    if !slot.accept(seq) {
                        continue;
                    }
                    self.apply_page_opened(pane_b, push_history, q.as_deref(), result);
                }
                WorkerEvt::Backlinks {
                    seq,
                    document_id,
                    result,
                } => {
                    if !self.pending.backlinks.accept(seq) {
                        continue;
                    }
                    if self.wiki_selected_id.as_deref() == Some(document_id.as_str()) {
                        match result {
                            Ok(backlinks) => {
                                self.wiki_backlinks = backlinks;
                                self.wiki_backlinks_error = None;
                            }
                            Err(error) => {
                                // Preserve the last known list, but make its
                                // potentially stale status explicit.
                                self.wiki_backlinks_error = Some(error);
                            }
                        }
                    }
                }
                WorkerEvt::Content {
                    seq,
                    node_id,
                    result,
                } => {
                    if !self.pending.content.accept(seq) {
                        continue;
                    }
                    // Selection moved on meanwhile: drop the answer.
                    if self.selected.as_deref() != Some(node_id.as_str()) {
                        continue;
                    }
                    match result {
                        Ok(body) => {
                            self.content = Some(body);
                            self.content_error = None;
                        }
                        Err(e) => {
                            self.content = None;
                            self.content_error = Some(e);
                        }
                    }
                }
                WorkerEvt::Expanded {
                    seq,
                    selected,
                    result,
                } => {
                    if !self.pending.expand.accept(seq) {
                        continue;
                    }
                    let merged = match result {
                        Ok(v) => v,
                        Err(e) => {
                            self.expand_note = Some(e);
                            continue;
                        }
                    };
                    let before = self.local_view.as_ref().map(|v| v.nodes.len()).unwrap_or(0);
                    let after = merged.nodes.len();
                    let max_n = self.max_nodes as usize;
                    if after == before {
                        self.expand_note = Some(format!(
                            "Для «{selected}» нет новых соседей в пределах max_nodes ({max_n})"
                        ));
                    } else if after >= max_n {
                        self.expand_note = Some(format!(
                            "Раскрытие заполнило вид до max_nodes ({max_n}); часть соседей может быть скрыта"
                        ));
                    }
                    if after > before {
                        self.expanded_dirty = true;
                    }
                    self.apply_local_topology(merged, /*reset_layout=*/ false);
                }
                WorkerEvt::SavedPage { seq, result } => {
                    if !self.pending.save.accept(seq) {
                        continue;
                    }
                    self.apply_save_result(result);
                }
                WorkerEvt::Activity { seq, result } => {
                    if !self.pending.activity.accept(seq) {
                        continue;
                    }
                    self.activity_last_refresh = Some(Instant::now());
                    match result {
                        Ok(items) => {
                            self.activity = items;
                            self.activity_error = None;
                        }
                        Err(error) => self.activity_error = Some(error),
                    }
                }
            }
        }
    }
}
