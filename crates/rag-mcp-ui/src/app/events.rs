//! Applies worker events to app state (seq-checked).

use super::*;
use crate::dto::BacklinkItem;
use crate::revisions::RevisionPage;

impl GraphApp {
    /// Drain worker results; stale seqs are ignored (a newer job superseded them).
    pub(super) fn drain_worker_events(&mut self) {
        while let Ok(evt) = self.worker.rx.try_recv() {
            self.apply_worker_event(evt);
        }
    }

    /// Route one worker answer to the handler of its domain.
    fn apply_worker_event(&mut self, evt: WorkerEvt) {
        match evt {
            WorkerEvt::GraphLoaded { seq, result } => self.on_graph_loaded(seq, result),
            WorkerEvt::ProjectCatalogLoaded { seq, result } => {
                self.on_project_catalog_loaded(seq, result)
            }
            WorkerEvt::ProjectHomeLoaded {
                seq,
                project,
                result,
            } => self.on_project_home_loaded(seq, project, result),
            WorkerEvt::LibraryLoaded {
                seq,
                project,
                result,
            } => self.on_library_loaded(seq, project, result),
            WorkerEvt::LibraryDocumentLoaded {
                seq,
                document_id,
                result,
            } => self.on_library_document_loaded(seq, document_id, result),
            WorkerEvt::SearchLoaded {
                seq,
                project,
                result,
            } => self.on_search_loaded(seq, project, result),
            WorkerEvt::OperationsLoaded { seq, result } => self.on_operations_loaded(seq, result),
            WorkerEvt::JobsLoaded { seq, result } => self.on_jobs_loaded(seq, result),
            WorkerEvt::JobChanged { seq, result } => self.on_job_changed(seq, result),
            WorkerEvt::MaintenanceCompleted { seq, result } => {
                self.on_maintenance_completed(seq, result)
            }
            WorkerEvt::RevisionsLoaded {
                seq,
                document_id,
                append,
                result,
            } => self.on_revisions_loaded(seq, document_id, append, result),
            WorkerEvt::RevisionSnapshotLoaded {
                seq,
                document_id,
                revision,
                result,
            } => self.on_revision_snapshot_loaded(seq, document_id, revision, result),
            WorkerEvt::RevisionDiffLoaded {
                seq,
                document_id,
                from_revision,
                result,
            } => self.on_revision_diff_loaded(seq, document_id, from_revision, result),
            WorkerEvt::RevisionRestored {
                seq,
                document_id,
                result,
            } => self.on_revision_restored(seq, document_id, result),
            WorkerEvt::WikiCatalog { seq, result } => self.on_wiki_catalog(seq, result),
            WorkerEvt::PageOpened {
                seq,
                pane_b,
                push_history,
                q,
                result,
            } => self.on_page_opened(seq, pane_b, push_history, q, result),
            WorkerEvt::Backlinks {
                seq,
                document_id,
                result,
            } => self.on_backlinks(seq, document_id, result),
            WorkerEvt::Content {
                seq,
                node_id,
                result,
            } => self.on_content(seq, node_id, result),
            WorkerEvt::Expanded {
                seq,
                selected,
                result,
            } => self.on_expanded(seq, selected, result),
            WorkerEvt::SavedPage { seq, result } => self.on_saved_page(seq, result),
            WorkerEvt::Activity { seq, result } => self.on_activity(seq, result),
        }
    }
}

/// Topology and project catalog.
impl GraphApp {
    fn on_graph_loaded(&mut self, seq: u64, result: Result<LoadedGraph, String>) {
        if !self.pending.graph.accept(seq) {
            return;
        }
        match result {
            Ok(loaded) => self.apply_loaded(loaded),
            Err(e) => {
                self.graph.load_error = Some(e);
                if self.mode == ViewMode::Wiki {
                    // Wiki can still answer (or fail with a clearer error).
                    self.ensure_wiki_loaded();
                }
            }
        }
    }

    fn on_project_catalog_loaded(&mut self, seq: u64, result: Result<Vec<String>, String>) {
        if !self.pending.project_catalog.accept(seq) {
            return;
        }
        match result {
            Ok(projects) => {
                self.scope.catalog = projects;
                self.scope.catalog_error = None;
            }
            Err(error) => self.scope.catalog_error = Some(error),
        }
    }

    fn on_content(&mut self, seq: u64, node_id: String, result: Result<DocumentBody, String>) {
        if !self.pending.content.accept(seq) {
            return;
        }
        // Selection moved on meanwhile: drop the answer.
        if self.graph.selected.as_deref() != Some(node_id.as_str()) {
            return;
        }
        match result {
            Ok(body) => {
                self.graph.content = Some(body);
                self.graph.content_error = None;
            }
            Err(e) => {
                self.graph.content = None;
                self.graph.content_error = Some(e);
            }
        }
    }

    fn on_expanded(&mut self, seq: u64, selected: String, result: Result<GraphView, String>) {
        if !self.pending.expand.accept(seq) {
            return;
        }
        let merged = match result {
            Ok(v) => v,
            Err(e) => {
                self.graph.expand_note = Some(e);
                return;
            }
        };
        let before = self
            .graph
            .local_view
            .as_ref()
            .map(|v| v.nodes.len())
            .unwrap_or(0);
        let after = merged.nodes.len();
        let max_n = self.graph.max_nodes as usize;
        if after == before {
            self.graph.expand_note = Some(format!(
                "Для «{selected}» нет новых соседей в пределах max_nodes ({max_n})"
            ));
        } else if after >= max_n {
            self.graph.expand_note = Some(format!(
                "Раскрытие заполнило вид до max_nodes ({max_n}); часть соседей может быть скрыта"
            ));
        }
        if after > before {
            self.graph.expanded_dirty = true;
        }
        self.apply_local_topology(merged, /*reset_layout=*/ false);
    }
}

/// Project Home, Library and Search.
impl GraphApp {
    fn on_project_home_loaded(
        &mut self,
        seq: u64,
        project: String,
        result: Result<ProjectHome, String>,
    ) {
        if !self.pending.project_home.accept(seq) {
            return;
        }
        if project != self.scope.wing.trim() {
            return;
        }
        self.home.project = Some(project);
        match result {
            Ok(home) => {
                self.home.overview = Some(home);
                self.home.error = None;
            }
            Err(error) => {
                self.home.overview = None;
                self.home.error = Some(error);
            }
        }
    }

    fn on_library_loaded(
        &mut self,
        seq: u64,
        project: Option<String>,
        result: Result<LibraryPage, String>,
    ) {
        if !self.pending.library.accept(seq) {
            return;
        }
        if !project_scope_matches(project.as_deref(), &self.scope.wing) {
            return;
        }
        match result {
            Ok(page) => {
                let selected_still_visible = self
                    .library
                    .selected_id
                    .as_ref()
                    .is_some_and(|selected| page.items.iter().any(|item| &item.id == selected));
                if !selected_still_visible {
                    self.library.selected_id = None;
                    self.library.document = None;
                    self.library.document_error = None;
                    self.pending.library_document.clear();
                }
                self.library.page = Some(page);
                self.library.error = None;
                if let Some(id) = self.library.open_after_load.take() {
                    if self
                        .library
                        .page
                        .as_ref()
                        .is_some_and(|page| page.items.iter().any(|item| item.id == id))
                    {
                        self.select_library_document(&id);
                    } else {
                        self.library.error =
                            Some(format!("Документ {id} не найден текущим фильтром каталога"));
                    }
                }
            }
            Err(error) => {
                self.library.page = None;
                self.library.error = Some(error);
            }
        }
    }

    fn on_library_document_loaded(
        &mut self,
        seq: u64,
        document_id: String,
        result: Result<DocumentBody, String>,
    ) {
        if !self.pending.library_document.accept(seq) {
            return;
        }
        if self.library.selected_id.as_deref() != Some(document_id.as_str()) {
            return;
        }
        match result {
            Ok(body) => {
                self.library.document = Some(body);
                self.library.document_error = None;
            }
            Err(error) => {
                self.library.document = None;
                self.library.document_error = Some(error);
            }
        }
    }

    fn on_search_loaded(
        &mut self,
        seq: u64,
        project: Option<String>,
        result: Result<SearchResults, String>,
    ) {
        if !self.pending.search.accept(seq) {
            return;
        }
        if !project_scope_matches(project.as_deref(), &self.scope.wing) {
            return;
        }
        match result {
            Ok(results) => {
                self.search.results = Some(results);
                self.search.error = None;
            }
            Err(error) => {
                self.search.results = None;
                self.search.error = Some(error);
            }
        }
    }
}

/// Operations, jobs and activity.
impl GraphApp {
    fn on_operations_loaded(&mut self, seq: u64, result: Result<OperationsSnapshot, String>) {
        if !self.pending.operations.accept(seq) {
            return;
        }
        match result {
            Ok(snapshot) => {
                self.ops.snapshot = Some(snapshot);
                self.ops.snapshot_stale = false;
                self.ops.error = None;
            }
            Err(error) => {
                self.ops.snapshot_stale = true;
                self.ops.error = Some(error);
            }
        }
    }

    fn on_jobs_loaded(&mut self, seq: u64, result: Result<Vec<JobSnapshot>, String>) {
        if !self.pending.jobs.accept(seq) {
            return;
        }
        self.ops.jobs_last_refresh = Some(Instant::now());
        match result {
            Ok(jobs) => {
                self.ops.jobs = jobs;
                self.ops.error = None;
            }
            Err(error) => self.ops.error = Some(error),
        }
    }

    fn on_job_changed(&mut self, seq: u64, result: Box<Result<JobSnapshot, String>>) {
        if !self.pending.operation_action.accept(seq) {
            return;
        }
        match *result {
            Ok(job) => {
                if let Some(existing) = self
                    .ops
                    .jobs
                    .iter_mut()
                    .find(|existing| existing.id == job.id)
                {
                    *existing = job;
                } else {
                    self.ops.jobs.insert(0, job);
                }
                self.ops.error = None;
                self.reload_jobs();
            }
            Err(error) => self.ops.error = Some(error),
        }
    }

    fn on_maintenance_completed(&mut self, seq: u64, result: Result<MaintenanceResult, String>) {
        if !self.pending.operation_action.accept(seq) {
            return;
        }
        match result {
            Ok(result) => {
                self.ops.last_result = Some(result);
                self.ops.error = None;
                self.reload_operations();
            }
            Err(error) => self.ops.error = Some(error),
        }
    }

    fn on_activity(&mut self, seq: u64, result: Result<Vec<ActivityEvent>, String>) {
        if !self.pending.activity.accept(seq) {
            return;
        }
        self.activity.last_refresh = Some(Instant::now());
        match result {
            Ok(items) => {
                self.activity.events = items;
                self.activity.error = None;
            }
            Err(error) => self.activity.error = Some(error),
        }
    }
}

/// Revisions.
impl GraphApp {
    fn on_revisions_loaded(
        &mut self,
        seq: u64,
        document_id: String,
        append: bool,
        result: Result<RevisionPage, String>,
    ) {
        if !self.pending.revisions.accept(seq) {
            return;
        }
        if self.revision.document_id.as_deref() != Some(document_id.as_str()) {
            return;
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

    fn on_revision_snapshot_loaded(
        &mut self,
        seq: u64,
        document_id: String,
        revision: i64,
        result: Result<DocumentBody, String>,
    ) {
        if !self.pending.revision_snapshot.accept(seq) {
            return;
        }
        if self.revision.document_id.as_deref() != Some(document_id.as_str())
            || self.revision.selected != Some(revision)
        {
            return;
        }
        match result {
            Ok(snapshot) => {
                self.revision.snapshot = Some(snapshot);
                self.revision.snapshot_error = None;
            }
            Err(error) => self.revision.snapshot_error = Some(error),
        }
    }

    fn on_revision_diff_loaded(
        &mut self,
        seq: u64,
        document_id: String,
        from_revision: i64,
        result: Result<RevisionDiff, String>,
    ) {
        if !self.pending.revision_diff.accept(seq) {
            return;
        }
        if self.revision.document_id.as_deref() != Some(document_id.as_str())
            || self.revision.selected != Some(from_revision)
        {
            return;
        }
        match result {
            Ok(diff) => {
                self.revision.diff = Some(diff);
                self.revision.diff_error = None;
            }
            Err(error) => self.revision.diff_error = Some(error),
        }
    }

    fn on_revision_restored(
        &mut self,
        seq: u64,
        document_id: String,
        result: Result<RestoreRevisionResult, String>,
    ) {
        if !self.pending.revision_restore.accept(seq) {
            return;
        }
        if self.revision.document_id.as_deref() != Some(document_id.as_str()) {
            return;
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
                if self.library.selected_id.as_deref() == self.revision.document_id.as_deref() {
                    let id = document_id.clone();
                    self.select_library_document(&id);
                }
            }
            Err(error) => self.revision.error = Some(error),
        }
    }
}

/// Wiki.
impl GraphApp {
    fn on_wiki_catalog(&mut self, seq: u64, result: Result<Vec<WikiPageMeta>, String>) {
        if !self.pending.catalog.accept(seq) {
            return;
        }
        self.wiki.loaded = true;
        match result {
            Ok(mut pages) => {
                sort_wiki_pages(&mut pages);
                self.wiki.pages = pages;
                self.wiki.pages_rev += 1;
                if let Some(id) = self.wiki.open_after_catalog.take() {
                    if self.wiki.pages.iter().any(|page| page.id == id) {
                        self.open_wiki_page_id(&id);
                    } else {
                        self.wiki.a.error =
                            Some(format!("Документа {id} нет в wiki-каталоге проекта"));
                    }
                } else {
                    self.auto_open_initial_page();
                }
            }
            Err(e) => self.wiki.a.error = Some(e),
        }
    }

    fn on_page_opened(
        &mut self,
        seq: u64,
        pane_b: bool,
        push_history: bool,
        q: Option<String>,
        result: Result<DocumentBody, String>,
    ) {
        let slot = if pane_b {
            &mut self.pending.page_b
        } else {
            &mut self.pending.page_a
        };
        if !slot.accept(seq) {
            return;
        }
        self.apply_page_opened(pane_b, push_history, q.as_deref(), result);
    }

    fn on_backlinks(
        &mut self,
        seq: u64,
        document_id: String,
        result: Result<Vec<BacklinkItem>, String>,
    ) {
        if !self.pending.backlinks.accept(seq) {
            return;
        }
        if self.wiki.a.selected_id.as_deref() == Some(document_id.as_str()) {
            match result {
                Ok(backlinks) => {
                    self.wiki.backlinks = backlinks;
                    self.wiki.backlinks_error = None;
                }
                Err(error) => {
                    // Preserve the last known list, but make its
                    // potentially stale status explicit.
                    self.wiki.backlinks_error = Some(error);
                }
            }
        }
    }

    fn on_saved_page(&mut self, seq: u64, result: Result<DocumentBody, String>) {
        if !self.pending.save.accept(seq) {
            return;
        }
        self.apply_save_result(result);
    }
}
