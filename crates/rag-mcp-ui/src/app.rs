//! GraphApp: eframe::App - native project, retrieval, document, wiki, and graph console.
//!
//! All blocking IO (HTTP / DuckDB) runs on the worker thread (`crate::worker`);
//! this file only dispatches [`WorkerCmd`] and applies [`WorkerEvt`] in `update()`.
//! Each job carries a `seq`; late answers whose seq no longer matches the pending
//! slot are dropped (race protection per EGUI_GRAPH_VIEW §2.5 / §8.3).

use egui::Vec2;
use rag_mcp::{GraphView, UI_GRAPH_EXPORT_MAX_NODES};

use crate::adapter::{adapt, topology_generation, AdaptOptions, GraphLens, UiGraph};
use crate::layout::{overview_grid_place, place_missing_near_neighbors, radial_place, PosCache};
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::load::{
    local_neighbors_bounded, resolve_exact_seed, resolve_seed, sort_wiki_pages, ActivityEvent,
    CliSource, DocumentBody, GatewayHealth, GraphSourceKind, LoadedGraph, OpenArgs, WikiPageMeta,
    WikiPutRequest, UI_HARD_MAX_NODES,
};
use crate::operations::{JobSnapshot, MaintenanceResult, OperationsSnapshot};
use crate::product::{LibraryItem, LibraryPage, LibraryRequest, ProjectHome};
use crate::revisions::{RestoreRevisionResult, RevisionDiff, RevisionItem};
use crate::search::{SearchRequest, SearchResults};
use crate::ui::canvas::draw_canvas;
use crate::ui::detail::{draw_detail, DetailAction, DetailContentState};
use crate::ui::empty::{
    draw_empty_banner, draw_no_source, EmptyGraphStats, EmptyKind, NoSourceAction,
};
use crate::ui::home::{draw_project_home, HomeAction};
use crate::ui::insights::{
    draw_evaluation_workspace, draw_models_workspace, EvaluationAction, ModelsAction,
};
use crate::ui::library::{
    draw_library_detail, draw_library_workspace, LibraryAction, LibraryDetailAction,
};
use crate::ui::operations::{
    draw_jobs, draw_maintenance, BackupForm, OperationsAction, OperationsTab, SyncJobForm,
};
use crate::ui::revisions::{draw_revisions_workspace, RevisionsAction, RevisionsView};
use crate::ui::search::{draw_search_workspace, SearchAction};
use crate::ui::shell::{draw_rail, draw_topbar, ShellRoute, TopbarState};
use crate::ui::status::draw_status;
use crate::ui::theme;
use crate::ui::wiki::{
    can_cancel_edit, content_summary_line, draw_wiki_edit_view, draw_wiki_info_panel,
    draw_wiki_read_view, draw_wiki_sidebar, slug_from_wiki_uri, wiki_filter_id, WikiEditBuffers,
    WikiReadContext,
};
use crate::worker::{LoadSource, WorkerCmd, WorkerEvt, WorkerHandle};

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn http_base_for_sources(
    loaded: Option<&GraphSourceKind>,
    configured: Option<&CliSource>,
) -> Option<String> {
    match loaded {
        Some(GraphSourceKind::HttpService { base }) => Some(base.clone()),
        _ => match configured {
            Some(CliSource::Http(base)) => Some(base.clone()),
            _ => None,
        },
    }
}

/// Product workspaces. Graph and Wiki retain their focused tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ViewMode {
    #[default]
    Home,
    Library,
    Search,
    Revisions,
    Wiki,
    Graph,
    Activity,
    Evaluation,
    Models,
}

/// Which wiki article column is focused (sidebar / links land here when dual-pane is on).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum WikiPane {
    #[default]
    A,
    B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseRisk {
    UnsavedWikiEdits,
    WikiSaveInFlight,
}

const fn close_risk(dirty_wiki_edit: bool, wiki_save_in_flight: bool) -> Option<CloseRisk> {
    if wiki_save_in_flight {
        Some(CloseRisk::WikiSaveInFlight)
    } else if dirty_wiki_edit {
        Some(CloseRisk::UnsavedWikiEdits)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GraphWikiTarget {
    CatalogId(String),
    WikiUri(String),
}

fn graph_wiki_target(
    document_id: Option<&str>,
    uri: Option<&str>,
    pages: &[WikiPageMeta],
) -> Option<GraphWikiTarget> {
    if let Some(page) = pages.iter().find(|page| {
        document_id.is_some_and(|id| page.id == id) || uri.is_some_and(|uri| page.uri == uri)
    }) {
        return Some(GraphWikiTarget::CatalogId(page.id.clone()));
    }
    uri.filter(|uri| uri.starts_with("wiki://"))
        .map(|uri| GraphWikiTarget::WikiUri(uri.to_string()))
}

fn seed_target_on_enter(
    view: Option<&GraphView>,
    input: &str,
    first_suggestion: Option<&str>,
) -> Option<String> {
    view.and_then(|view| resolve_exact_seed(view, input))
        .or_else(|| first_suggestion.map(str::to_string))
}

fn mode_after_project_switch(mode: ViewMode) -> ViewMode {
    if mode == ViewMode::Revisions {
        ViewMode::Library
    } else {
        mode
    }
}

fn is_operations_mutation(action: &OperationsAction) -> bool {
    matches!(
        action,
        OperationsAction::StartSync(_)
            | OperationsAction::CancelJob(_)
            | OperationsAction::Checkpoint
            | OperationsAction::Backup(_)
    )
}

fn project_scope_matches(response_project: Option<&str>, current_project: &str) -> bool {
    let current_project = current_project.trim();
    response_project == (!current_project.is_empty()).then_some(current_project)
}

fn reconcile_project_change(
    selected_project: &mut String,
    previous_project: &str,
    mutation_in_flight: bool,
) -> bool {
    let changed = selected_project != previous_project;
    if changed && mutation_in_flight {
        previous_project.clone_into(selected_project);
        false
    } else {
        changed
    }
}

const fn has_noncancellable_mutation(
    save_in_flight: bool,
    operation_in_flight: bool,
    restore_in_flight: bool,
) -> bool {
    save_in_flight || operation_in_flight || restore_in_flight
}

fn prepare_library_request_from_search(
    request: &mut LibraryRequest,
    project: &str,
    title: &str,
    uri: &str,
    include_archived: bool,
) {
    request.clear_filters();
    request.wing = project.trim().to_string();
    request.include_archived = include_archived;
    request.q = if uri.trim().is_empty() {
        title.to_string()
    } else {
        uri.to_string()
    };
}

#[derive(Debug, Default)]
struct RevisionWorkspace {
    document_id: Option<String>,
    document_title: String,
    document_uri: String,
    document_layer: String,
    head: Option<i64>,
    items: Vec<RevisionItem>,
    total: u64,
    next_cursor: Option<String>,
    selected: Option<i64>,
    snapshot: Option<DocumentBody>,
    snapshot_error: Option<String>,
    diff: Option<RevisionDiff>,
    diff_error: Option<String>,
    restore_confirm: Option<i64>,
    restore_result: Option<RestoreRevisionResult>,
    error: Option<String>,
}

impl RevisionWorkspace {
    fn clear(&mut self) {
        *self = Self::default();
    }
}

pub struct GraphApp {
    open: OpenArgs,
    worker: WorkerHandle,
    /// Monotonic job counter; every pending slot stores the seq it waits for.
    seq: u64,
    pending_graph: Option<u64>,
    pending_project_catalog: Option<u64>,
    pending_catalog: Option<u64>,
    pending_page_a: Option<u64>,
    pending_page_b: Option<u64>,
    pending_backlinks: Option<u64>,
    pending_content: Option<u64>,
    pending_expand: Option<u64>,
    pending_save: Option<u64>,
    pending_activity: Option<u64>,
    pending_project_home: Option<u64>,
    pending_library: Option<u64>,
    pending_library_document: Option<u64>,
    pending_search: Option<u64>,
    pending_operations: Option<u64>,
    pending_jobs: Option<u64>,
    pending_operation_action: Option<u64>,
    pending_revisions: Option<u64>,
    pending_revision_snapshot: Option<u64>,
    pending_revision_diff: Option<u64>,
    pending_revision_restore: Option<u64>,
    /// HTTP base URL editable on the no-source start screen.
    connect_url: String,
    close_confirmation_open: bool,
    force_close: bool,

    full_view: Option<GraphView>,
    project_catalog: Vec<String>,
    project_catalog_error: Option<String>,
    /// True after the first successful source bootstrap. This distinguishes an
    /// initial default from the user's later explicit "All projects" choice.
    source: Option<GraphSourceKind>,
    load_error: Option<String>,
    raw_truncated: bool,
    raw_node_count: usize,
    ops_health: Option<GatewayHealth>,
    activity: Vec<ActivityEvent>,
    activity_error: Option<String>,
    activity_last_refresh: Option<Instant>,
    activity_auto_refresh: bool,
    activity_filter: String,
    activity_kind_filter: String,
    activity_client_filter: String,
    activity_action_filter: String,
    activity_status_filter: String,

    // --- Project Home + Unified Library state ---
    project_home: Option<ProjectHome>,
    project_home_project: Option<String>,
    project_home_error: Option<String>,
    library_request: LibraryRequest,
    library_page: Option<LibraryPage>,
    library_error: Option<String>,
    library_cursor_history: Vec<Option<String>>,
    library_selected_id: Option<String>,
    library_open_after_load: Option<String>,
    library_document: Option<DocumentBody>,
    library_document_error: Option<String>,
    search_request: SearchRequest,
    search_results: Option<SearchResults>,
    search_error: Option<String>,

    // --- Operations state ---
    operations_tab: OperationsTab,
    operations_snapshot: Option<OperationsSnapshot>,
    /// The last health snapshot is retained for diagnosis, but must never be
    /// presented as current after a failed status/doctor refresh.
    operations_snapshot_stale: bool,
    operations_jobs: Vec<JobSnapshot>,
    operations_error: Option<String>,
    operations_last_result: Option<MaintenanceResult>,
    operations_jobs_last_refresh: Option<Instant>,
    sync_job_form: SyncJobForm,
    backup_form: BackupForm,

    // --- Revision workspace state ---
    revision: RevisionWorkspace,

    mode: ViewMode,

    seed_input: String,
    seed_id: Option<String>,
    seed_error: Option<String>,
    depth: u32,
    max_nodes: u32,
    graph_lens: GraphLens,

    show_tags: bool,
    show_stubs: bool,
    prev_show_tags: bool,
    prev_show_stubs: bool,
    filter_wing: String,
    filter_room: String,
    prev_filter_wing: String,
    prev_filter_room: String,

    /// Local topology (seed BFS and/or Expand merges) before adapter filters.
    local_view: Option<GraphView>,
    /// Exact local BFS budget truncation, independent from server/adapter caps.
    local_truncated: bool,
    ui_graph: Option<UiGraph>,
    positions: PosCache,
    layout_ready: bool,
    need_fit: bool,
    last_topo: u64,
    /// Banner after expand hit max_nodes or added nothing.
    expand_note: Option<String>,
    /// True when `local_view` contains Expand merges beyond the plain seed BFS
    /// (gates the "Reset to seed" confirmation).
    expanded_dirty: bool,
    /// "Reset to seed" confirmation popup is open.
    confirm_reset: bool,

    selected: Option<String>,
    /// Full wiki/raw body for the selected node (HTTP or --db).
    content: Option<DocumentBody>,
    content_error: Option<String>,
    pan: Vec2,
    zoom: f32,

    // --- Wiki browser state ---
    wiki_pages: Vec<WikiPageMeta>,
    wiki_filter: String,
    wiki_show_summaries: bool,
    wiki_sidebar_visible: bool,
    wiki_info_visible: bool,
    wiki_selected_id: Option<String>,
    /// Unified Library page to open after a project-scoped wiki catalog refresh.
    wiki_open_after_catalog: Option<String>,
    wiki_article: Option<DocumentBody>,
    wiki_error: Option<String>,
    wiki_loaded: bool,
    /// History stack of wiki page ids for Back (Obsidian-like).
    wiki_history: Vec<String>,
    wiki_backlinks: Vec<crate::load::BacklinkItem>,
    wiki_backlinks_error: Option<String>,
    /// In-app editor buffers (None = read mode). Applies to pane A only.
    wiki_edit: Option<WikiEditBuffers>,
    /// Last successful save note (status strip).
    wiki_save_note: Option<String>,
    /// Two-article layout: catalog left, pane A center, pane B right SidePanel.
    wiki_dual_pane: bool,
    /// Focused column for sidebar clicks and wikilink navigation.
    wiki_focus: WikiPane,
    // --- Secondary article pane (B) ---
    wiki_selected_id_b: Option<String>,
    wiki_article_b: Option<DocumentBody>,
    wiki_error_b: Option<String>,
}

impl GraphApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: OpenArgs) -> Self {
        theme::install(&cc.egui_ctx);
        let depth = open.depth.clamp(1, 3);
        let max_nodes = open.max_nodes.clamp(1, UI_GRAPH_EXPORT_MAX_NODES);
        let seed_input = open.seed.clone().unwrap_or_default();
        let connect_url = match open.source.as_ref() {
            Some(CliSource::Http(base)) => base.clone(),
            _ => "http://127.0.0.1:7432".into(),
        };
        let mut app = Self {
            open,
            worker: crate::worker::spawn(cc.egui_ctx.clone()),
            seq: 0,
            pending_graph: None,
            pending_project_catalog: None,
            pending_catalog: None,
            pending_page_a: None,
            pending_page_b: None,
            pending_backlinks: None,
            pending_content: None,
            pending_expand: None,
            pending_save: None,
            pending_activity: None,
            pending_project_home: None,
            pending_library: None,
            pending_library_document: None,
            pending_search: None,
            pending_operations: None,
            pending_jobs: None,
            pending_operation_action: None,
            pending_revisions: None,
            pending_revision_snapshot: None,
            pending_revision_diff: None,
            pending_revision_restore: None,
            connect_url,
            close_confirmation_open: false,
            force_close: false,
            full_view: None,
            project_catalog: Vec::new(),
            project_catalog_error: None,
            source: None,
            load_error: None,
            raw_truncated: false,
            raw_node_count: 0,
            ops_health: None,
            activity: Vec::new(),
            activity_error: None,
            activity_last_refresh: None,
            activity_auto_refresh: true,
            activity_filter: String::new(),
            activity_kind_filter: "all".into(),
            activity_client_filter: String::new(),
            activity_action_filter: String::new(),
            activity_status_filter: "all".into(),
            project_home: None,
            project_home_project: None,
            project_home_error: None,
            library_request: LibraryRequest {
                limit: 50,
                ..LibraryRequest::default()
            },
            library_page: None,
            library_error: None,
            library_cursor_history: Vec::new(),
            library_selected_id: None,
            library_open_after_load: None,
            library_document: None,
            library_document_error: None,
            search_request: SearchRequest::default(),
            search_results: None,
            search_error: None,
            operations_tab: OperationsTab::Activity,
            operations_snapshot: None,
            operations_snapshot_stale: false,
            operations_jobs: Vec::new(),
            operations_error: None,
            operations_last_result: None,
            operations_jobs_last_refresh: None,
            sync_job_form: SyncJobForm::default(),
            backup_form: BackupForm::default(),
            revision: RevisionWorkspace::default(),
            mode: ViewMode::Home,
            seed_input,
            seed_id: None,
            seed_error: None,
            depth,
            max_nodes,
            graph_lens: GraphLens::Neighborhood,
            show_tags: false,
            show_stubs: true,
            prev_show_tags: false,
            prev_show_stubs: true,
            filter_wing: String::new(),
            filter_room: String::new(),
            prev_filter_wing: String::new(),
            prev_filter_room: String::new(),
            local_view: None,
            local_truncated: false,
            ui_graph: None,
            positions: PosCache::default(),
            layout_ready: false,
            need_fit: true,
            last_topo: 0,
            expand_note: None,
            expanded_dirty: false,
            confirm_reset: false,
            selected: None,
            content: None,
            content_error: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            wiki_pages: Vec::new(),
            wiki_filter: String::new(),
            wiki_show_summaries: false,
            wiki_sidebar_visible: true,
            wiki_info_visible: true,
            wiki_selected_id: None,
            wiki_open_after_catalog: None,
            wiki_article: None,
            wiki_error: None,
            wiki_loaded: false,
            wiki_history: Vec::new(),
            wiki_backlinks: Vec::new(),
            wiki_backlinks_error: None,
            wiki_edit: None,
            wiki_save_note: None,
            wiki_dual_pane: false,
            wiki_focus: WikiPane::A,
            wiki_selected_id_b: None,
            wiki_article_b: None,
            wiki_error_b: None,
        };
        app.dispatch_graph_load();
        app
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn any_pending(&self) -> bool {
        self.pending_graph.is_some()
            || self.pending_project_catalog.is_some()
            || self.pending_catalog.is_some()
            || self.pending_page_a.is_some()
            || self.pending_page_b.is_some()
            || self.pending_backlinks.is_some()
            || self.pending_content.is_some()
            || self.pending_expand.is_some()
            || self.pending_save.is_some()
            || self.pending_activity.is_some()
            || self.pending_project_home.is_some()
            || self.pending_library.is_some()
            || self.pending_library_document.is_some()
            || self.pending_search.is_some()
            || self.pending_operations.is_some()
            || self.pending_jobs.is_some()
            || self.pending_operation_action.is_some()
            || self.pending_revisions.is_some()
            || self.pending_revision_snapshot.is_some()
            || self.pending_revision_diff.is_some()
            || self.pending_revision_restore.is_some()
    }

    fn noncancellable_mutation_in_flight(&self) -> bool {
        has_noncancellable_mutation(
            self.pending_save.is_some(),
            self.pending_operation_action.is_some(),
            self.pending_revision_restore.is_some(),
        )
    }

    fn mutation_status_label(&self) -> Option<&'static str> {
        if self.pending_save.is_some() {
            Some("сохранение…")
        } else if self.pending_revision_restore.is_some() {
            Some("восстановление…")
        } else if self.pending_operation_action.is_some() {
            Some("операция…")
        } else {
            None
        }
    }

    fn shell_route(&self) -> ShellRoute {
        match self.mode {
            ViewMode::Home => ShellRoute::Console,
            ViewMode::Library | ViewMode::Revisions => ShellRoute::Corpus,
            ViewMode::Search => ShellRoute::Search,
            ViewMode::Graph => ShellRoute::Graph,
            ViewMode::Wiki => ShellRoute::Wiki,
            ViewMode::Activity => ShellRoute::Agents,
            ViewMode::Evaluation => ShellRoute::Evaluation,
            ViewMode::Models => ShellRoute::Models,
        }
    }

    fn activate_shell_route(&mut self, route: ShellRoute) {
        if route.requires_http() && self.activity_base().is_none() {
            return;
        }
        if route == ShellRoute::Wiki && self.available_load_source().is_none() {
            return;
        }
        match route {
            ShellRoute::Console => {
                self.mode = ViewMode::Home;
                self.ensure_project_home_loaded();
                if self.operations_snapshot.is_none() && self.pending_operations.is_none() {
                    self.reload_operations();
                }
                if self.pending_activity.is_none() {
                    self.refresh_activity();
                }
            }
            ShellRoute::Corpus => {
                self.mode = ViewMode::Library;
                self.ensure_library_loaded();
            }
            ShellRoute::Search => {
                self.mode = ViewMode::Search;
                self.search_request.wing = nonempty(&self.filter_wing);
            }
            ShellRoute::Graph => self.mode = ViewMode::Graph,
            ShellRoute::Wiki => {
                self.mode = ViewMode::Wiki;
                self.ensure_wiki_loaded();
            }
            ShellRoute::Agents => {
                self.mode = ViewMode::Activity;
                self.operations_tab = OperationsTab::Activity;
                self.refresh_activity();
            }
            ShellRoute::Evaluation => {
                self.mode = ViewMode::Evaluation;
                self.refresh_activity();
            }
            ShellRoute::Models => {
                self.mode = ViewMode::Models;
                if self.operations_snapshot.is_none() && self.pending_operations.is_none() {
                    self.reload_operations();
                }
            }
        }
    }

    fn shell_health(&self) -> (Option<bool>, String) {
        if self.activity_base().is_none() {
            if self.load_error.is_some() && self.shell_read_only_source().is_some() {
                return (None, "источник только для чтения не загружен".into());
            }
            return (
                None,
                self.shell_read_only_source()
                    .unwrap_or("gateway · офлайн")
                    .into(),
            );
        }
        if self.pending_operations.is_some() {
            return (None, "gateway · обновление состояния…".into());
        }
        if self.operations_snapshot_stale {
            return (None, "gateway · состояние устарело".into());
        }
        if self.operations_error.is_some() {
            return (None, "gateway · требуется внимание".into());
        }
        if let Some(snapshot) = &self.operations_snapshot {
            let healthy = snapshot.doctor.ok && snapshot.status.ready_for_search;
            return (
                Some(healthy),
                format!(
                    "{} · schema v{} · fts {}",
                    snapshot.status.backend,
                    snapshot.status.schema_version,
                    if snapshot.status.fts_ready { "ok" } else { "!" }
                ),
            );
        }
        if let Some(health) = &self.ops_health {
            let healthy = health.fts_ready
                && health.relational_integrity_ok
                && !health.wal_too_large
                && health.documents_without_chunks == 0;
            return (
                Some(healthy),
                format!(
                    "{} · schema v{} · fts {}",
                    health.backend,
                    health.schema_version,
                    if health.fts_ready { "ok" } else { "!" }
                ),
            );
        }
        (None, "gateway · проверка состояния…".into())
    }

    fn shell_read_only_source(&self) -> Option<&'static str> {
        match self.source.as_ref() {
            Some(GraphSourceKind::LiveStore { .. }) => Some("duckdb · только чтение"),
            Some(GraphSourceKind::SnapshotFile { .. }) => Some("snapshot · только чтение"),
            Some(GraphSourceKind::VaultGraphJson { .. }) => Some("vault graph · только чтение"),
            Some(GraphSourceKind::HttpService { .. }) => None,
            None => match self.open.source.as_ref() {
                Some(CliSource::Db(_)) => Some("duckdb · загрузка только для чтения"),
                Some(CliSource::Snapshot(_)) => Some("snapshot · загрузка только для чтения"),
                Some(CliSource::Http(_)) | None => None,
            },
        }
    }

    fn activity_base(&self) -> Option<String> {
        // Product APIs remain usable while the graph endpoint is busy or
        // unavailable; only Connections depends on topology.
        http_base_for_sources(self.source.as_ref(), self.open.source.as_ref())
    }

    fn topology_query_requires_reload(&self) -> bool {
        self.activity_base().is_some()
            || matches!(self.source, Some(GraphSourceKind::LiveStore { .. }))
            || matches!(self.open.source, Some(CliSource::Db(_)))
    }

    fn available_load_source(&self) -> Option<LoadSource> {
        self.source
            .as_ref()
            .and_then(LoadSource::from_graph_source)
            .or_else(|| match self.open.source.as_ref() {
                // A configured HTTP gateway is sufficient for document/wiki
                // APIs even before the independent graph load succeeds.
                Some(CliSource::Http(base)) => Some(LoadSource::Http(base.clone())),
                _ => None,
            })
    }

    fn reload_project_catalog(&mut self) {
        if self.pending_project_catalog.is_some() {
            return;
        }
        let Some(base) = self.activity_base() else {
            self.project_catalog_error =
                Some("Для каталога проектов требуется HTTP gateway".into());
            return;
        };
        self.project_catalog_error = None;
        let seq = self.next_seq();
        self.pending_project_catalog = Some(seq);
        self.worker
            .send(WorkerCmd::LoadProjectCatalog { seq, base });
    }

    fn refresh_activity(&mut self) {
        if self.pending_activity.is_some() {
            return;
        }
        let Some(base) = self.activity_base() else {
            self.activity_error = Some("Для журнала требуется HTTP gateway".into());
            return;
        };
        let seq = self.next_seq();
        self.pending_activity = Some(seq);
        self.worker.send(WorkerCmd::LoadActivity { seq, base });
    }

    fn reload_project_home(&mut self) {
        let project = self.filter_wing.trim().to_string();
        self.project_home = None;
        self.project_home_error = None;
        self.project_home_project = None;
        if project.is_empty() {
            self.pending_project_home = None;
            return;
        }
        let Some(base) = self.activity_base() else {
            self.project_home_error = Some("Для пульта проекта требуется HTTP gateway".into());
            return;
        };
        let seq = self.next_seq();
        self.pending_project_home = Some(seq);
        self.worker
            .send(WorkerCmd::LoadProjectHome { seq, base, project });
    }

    fn ensure_project_home_loaded(&mut self) {
        let project = nonempty(&self.filter_wing);
        if self.pending_project_home.is_none() && self.project_home_project != project {
            self.reload_project_home();
        }
    }

    fn reload_library(&mut self) {
        let Some(base) = self.activity_base() else {
            self.library_error = Some("Для корпуса требуется HTTP gateway".into());
            self.library_page = None;
            return;
        };
        self.library_error = None;
        self.library_request.wing = self.filter_wing.trim().to_string();
        self.library_request.limit = self.library_request.limit.clamp(1, 200);
        let request = self.library_request.clone();
        let seq = self.next_seq();
        self.pending_library = Some(seq);
        self.worker
            .send(WorkerCmd::LoadLibrary { seq, base, request });
    }

    fn ensure_library_loaded(&mut self) {
        if self.pending_library.is_none() && self.library_page.is_none() {
            self.reload_library();
        }
    }

    fn reset_library_for_filters(&mut self) {
        self.library_request.cursor = None;
        self.library_cursor_history.clear();
        self.library_selected_id = None;
        self.library_document = None;
        self.library_document_error = None;
        self.pending_library_document = None;
        self.reload_library();
    }

    fn select_library_document(&mut self, id: &str) {
        let item = self
            .library_page
            .as_ref()
            .and_then(|page| page.items.iter().find(|item| item.id == id))
            .cloned();
        let Some(item) = item else {
            return;
        };
        self.library_selected_id = Some(item.id.clone());
        self.library_document = None;
        self.library_document_error = None;
        let Some(base) = self.activity_base() else {
            self.library_document_error =
                Some("Для превью документа требуется HTTP gateway".into());
            return;
        };
        let seq = self.next_seq();
        self.pending_library_document = Some(seq);
        self.worker.send(WorkerCmd::LoadLibraryDocument {
            seq,
            base,
            document_id: item.id,
            uri: item.uri,
        });
    }

    fn run_search(&mut self) {
        if self.search_request.query.trim().is_empty() {
            self.search_error = Some("Введите поисковый запрос".into());
            return;
        }
        let Some(base) = self.activity_base() else {
            self.search_error = Some("Для поиска требуется HTTP gateway".into());
            return;
        };
        self.search_request.wing = nonempty(&self.filter_wing);
        self.search_request.top_k = self.search_request.top_k.clamp(1, 100);
        self.search_error = None;
        let seq = self.next_seq();
        self.pending_search = Some(seq);
        self.worker.send(WorkerCmd::Search {
            seq,
            base,
            request: self.search_request.clone(),
        });
    }

    fn reload_operations(&mut self) {
        let Some(base) = self.activity_base() else {
            self.operations_error = Some("Для операций требуется HTTP gateway".into());
            return;
        };
        self.operations_error = None;
        let seq = self.next_seq();
        self.pending_operations = Some(seq);
        self.worker.send(WorkerCmd::LoadOperations { seq, base });
    }

    fn reload_jobs(&mut self) {
        if self.pending_jobs.is_some() {
            return;
        }
        let Some(base) = self.activity_base() else {
            self.operations_error = Some("Для фоновых задач требуется HTTP gateway".into());
            return;
        };
        let seq = self.next_seq();
        self.pending_jobs = Some(seq);
        self.worker.send(WorkerCmd::LoadJobs { seq, base });
    }

    fn dispatch_operations_action(&mut self, action: OperationsAction) {
        if is_operations_mutation(&action) && self.pending_operation_action.is_some() {
            return;
        }
        match action {
            OperationsAction::None => {}
            OperationsAction::RefreshHealth => self.reload_operations(),
            OperationsAction::RefreshJobs => self.reload_jobs(),
            OperationsAction::StartSync(request) => {
                let Some(base) = self.activity_base() else {
                    self.operations_error = Some("Для фоновых задач требуется HTTP gateway".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker
                    .send(WorkerCmd::StartSyncJob { seq, base, request });
            }
            OperationsAction::CancelJob(id) => {
                let Some(base) = self.activity_base() else {
                    self.operations_error = Some("Для фоновых задач требуется HTTP gateway".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker.send(WorkerCmd::CancelJob { seq, base, id });
            }
            OperationsAction::Checkpoint => {
                let Some(base) = self.activity_base() else {
                    self.operations_error = Some("Для обслуживания требуется HTTP gateway".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker.send(WorkerCmd::Checkpoint { seq, base });
            }
            OperationsAction::Backup(request) => {
                let Some(base) = self.activity_base() else {
                    self.operations_error = Some("Для обслуживания требуется HTTP gateway".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker.send(WorkerCmd::Backup { seq, base, request });
            }
        }
    }

    fn open_revisions(&mut self, item: &LibraryItem) {
        if self.pending_revision_restore.is_some() {
            self.mode = ViewMode::Revisions;
            self.revision.error = Some(
                "Идёт восстановление. Дождитесь результата перед открытием другой истории.".into(),
            );
            return;
        }
        self.revision.clear();
        self.revision.document_id = Some(item.id.clone());
        self.revision.document_title = item.title.clone();
        self.revision.document_uri = item.uri.clone();
        self.revision.document_layer = item.layer.clone();
        self.revision.head = self
            .library_document
            .as_ref()
            .filter(|body| body.id == item.id)
            .and_then(|body| body.revision);
        self.mode = ViewMode::Revisions;
        self.reload_revisions();
    }

    fn reload_revisions(&mut self) {
        self.revision.selected = None;
        self.revision.snapshot = None;
        self.revision.snapshot_error = None;
        self.revision.diff = None;
        self.revision.diff_error = None;
        self.revision.restore_confirm = None;
        self.request_revisions(None, false);
    }

    fn load_more_revisions(&mut self) {
        let Some(cursor) = self.revision.next_cursor.clone() else {
            return;
        };
        self.request_revisions(Some(cursor), true);
    }

    fn request_revisions(&mut self, cursor: Option<String>, append: bool) {
        let Some(document_id) = self.revision.document_id.clone() else {
            self.revision.error = Some("Откройте историю из документа в корпусе".into());
            return;
        };
        let Some(base) = self.activity_base() else {
            self.revision.error = Some("Для истории версий требуется HTTP gateway".into());
            return;
        };
        self.revision.error = None;
        if !append {
            self.revision.next_cursor = None;
        }
        let seq = self.next_seq();
        self.pending_revisions = Some(seq);
        self.worker.send(WorkerCmd::LoadRevisions {
            seq,
            base,
            document_id,
            cursor,
            append,
        });
    }

    fn load_revision_details(&mut self, from_revision: i64) {
        let Some(document_id) = self.revision.document_id.clone() else {
            return;
        };
        let Some(base) = self.activity_base() else {
            self.revision.error = Some("Для деталей версии требуется HTTP gateway".into());
            return;
        };
        self.revision.selected = Some(from_revision);
        self.revision.snapshot = None;
        self.revision.snapshot_error = None;
        self.revision.diff = None;
        self.revision.diff_error = None;
        let snapshot_seq = self.next_seq();
        self.pending_revision_snapshot = Some(snapshot_seq);
        self.worker.send(WorkerCmd::LoadRevisionSnapshot {
            seq: snapshot_seq,
            base: base.clone(),
            document_id: document_id.clone(),
            revision: from_revision,
        });
        let diff_seq = self.next_seq();
        self.pending_revision_diff = Some(diff_seq);
        self.worker.send(WorkerCmd::LoadRevisionDiff {
            seq: diff_seq,
            base,
            document_id,
            from_revision,
            to_revision: None,
        });
    }

    fn restore_revision(&mut self, revision: i64) {
        let selected_is_raw = self
            .revision
            .items
            .iter()
            .find(|item| item.revision == revision)
            .is_some_and(|item| item.layer == "raw");
        if self.revision.document_layer == "raw" || selected_is_raw {
            self.revision.error = Some(
                "Raw-документы управляются исходным файлом. Восстановите файл и синхронизируйте его, чтобы создать новую индексированную версию."
                    .into(),
            );
            self.revision.restore_confirm = None;
            return;
        }
        let (Some(document_id), Some(if_match_revision), Some(base)) = (
            self.revision.document_id.clone(),
            self.revision.head,
            self.activity_base(),
        ) else {
            self.revision.error =
                Some("Для восстановления нужны текущая версия документа и HTTP gateway".into());
            return;
        };
        self.revision.error = None;
        let seq = self.next_seq();
        self.pending_revision_restore = Some(seq);
        self.worker.send(WorkerCmd::RestoreRevision {
            seq,
            base,
            document_id,
            revision,
            if_match_revision,
        });
    }

    fn reset_product_for_project(&mut self) {
        self.worker.cancel_project_scoped_reads();
        self.mode = mode_after_project_switch(self.mode);
        self.pending_graph = None;
        self.pending_project_home = None;
        self.pending_library = None;
        self.pending_library_document = None;
        self.pending_search = None;
        self.pending_content = None;
        self.pending_expand = None;
        self.revision.clear();
        self.pending_revisions = None;
        self.pending_revision_snapshot = None;
        self.pending_revision_diff = None;
        self.project_home = None;
        self.project_home_project = None;
        self.project_home_error = None;
        self.library_request.wing = self.filter_wing.trim().to_string();
        self.library_page = None;
        self.library_error = None;
        self.library_request.cursor = None;
        self.library_cursor_history.clear();
        self.library_selected_id = None;
        self.library_open_after_load = None;
        self.library_document = None;
        self.library_document_error = None;
        self.search_request.wing = nonempty(&self.filter_wing);
        self.search_results = None;
        self.search_error = None;
        match self.mode {
            ViewMode::Home => self.reload_project_home(),
            ViewMode::Library => self.reload_library(),
            ViewMode::Search
            | ViewMode::Revisions
            | ViewMode::Wiki
            | ViewMode::Graph
            | ViewMode::Activity
            | ViewMode::Evaluation
            | ViewMode::Models => {}
        }
    }

    fn selected_library_item(&self) -> Option<LibraryItem> {
        let selected = self.library_selected_id.as_deref()?;
        self.library_page
            .as_ref()?
            .items
            .iter()
            .find(|item| item.id == selected)
            .cloned()
    }

    /// Kick the initial / retry topology load on the worker (no-op without a source).
    fn dispatch_graph_load(&mut self) {
        let Some(src) = self.open.source.clone() else {
            return;
        };
        let refresh_project_catalog = matches!(&src, CliSource::Http(_));
        self.worker.cancel_topology_reads();
        // A request can change project, tag inclusion or focus. Do not paint the
        // previous topology under the new controls while the worker is pending.
        self.load_error = None;
        self.pending_expand = None;
        self.pending_content = None;
        self.full_view = None;
        self.local_view = None;
        self.local_truncated = false;
        self.ui_graph = None;
        self.positions.clear();
        self.layout_ready = false;
        self.need_fit = true;
        self.selected = None;
        self.content = None;
        self.content_error = None;
        self.expand_note = None;
        self.expanded_dirty = false;
        self.raw_truncated = false;
        self.raw_node_count = 0;
        let seq = self.next_seq();
        self.pending_graph = Some(seq);
        if refresh_project_catalog {
            self.reload_project_catalog();
        }
        self.worker.send(WorkerCmd::LoadGraph {
            seq,
            source: src,
            seed: nonempty(&self.seed_input),
            depth: self.depth,
            project: nonempty(&self.filter_wing),
            include_tags: self.show_tags,
        });
    }

    fn submit_graph_focus(&mut self) {
        self.graph_lens = GraphLens::Neighborhood;
        if self.topology_query_requires_reload() {
            self.seed_id = None;
            self.seed_error = None;
            self.dispatch_graph_load();
        } else {
            self.apply_seed_from_input();
        }
    }

    fn open_graph_focus(&mut self, query: String, fallback_label: Option<String>) {
        self.seed_input = query;
        self.mode = ViewMode::Graph;
        self.submit_graph_focus();
        if self.activity_base().is_none() && self.seed_error.is_some() {
            if let Some(fallback) = fallback_label {
                self.seed_input = fallback;
                self.submit_graph_focus();
            }
        }
    }

    /// Switch to an HTTP source from the no-source start screen (no restart).
    fn connect_http(&mut self) {
        let url = self.connect_url.trim().to_string();
        if url.is_empty() {
            self.load_error = Some("HTTP URL не задан".into());
            return;
        }
        self.connect_url.clone_from(&url);
        self.open.source = Some(CliSource::Http(url));
        self.load_error = None;
        self.project_catalog.clear();
        self.project_catalog_error = None;
        self.wiki_loaded = false;
        self.project_home = None;
        self.project_home_project = None;
        self.library_page = None;
        self.dispatch_graph_load();
    }

    fn apply_loaded(&mut self, loaded: LoadedGraph) {
        self.load_error = None;
        self.raw_truncated = loaded.truncated;
        self.raw_node_count = loaded.raw_node_count;
        self.ops_health = loaded.health;
        if let Some(projects) = loaded.projects {
            self.project_catalog = projects;
            self.project_catalog_error = None;
        }
        self.source = Some(loaded.source);
        self.full_view = Some(loaded.view);
        if !self.seed_input.is_empty() {
            self.apply_seed_from_input();
        } else {
            self.rebuild_ui_graph();
        }
        match self.mode {
            ViewMode::Home => {
                self.reload_project_home();
                self.reload_operations();
                self.refresh_activity();
            }
            ViewMode::Library => self.reload_library(),
            ViewMode::Wiki => self.ensure_wiki_loaded(),
            ViewMode::Activity => self.refresh_activity(),
            ViewMode::Models => self.reload_operations(),
            ViewMode::Evaluation => self.refresh_activity(),
            ViewMode::Search | ViewMode::Revisions | ViewMode::Graph => {}
        }
    }

    fn ensure_wiki_loaded(&mut self) {
        if self.wiki_loaded || self.pending_catalog.is_some() {
            return;
        }
        self.reload_wiki_catalog();
    }

    fn reload_wiki_catalog(&mut self) {
        self.wiki_error = None;
        let Some(source) = self.available_load_source() else {
            self.wiki_loaded = true;
            self.wiki_error = Some(if self.open.source.is_some() {
                "В snapshot-режиме нет каталога вики; используйте --http или --db".into()
            } else {
                "Источник данных не выбран".into()
            });
            return;
        };
        let seq = self.next_seq();
        self.pending_catalog = Some(seq);
        self.worker.send(WorkerCmd::LoadWikiCatalog {
            seq,
            source,
            project: nonempty(&self.filter_wing),
        });
    }

    /// Default page after the catalog lands: seed title match, else overview/first.
    fn auto_open_initial_page(&mut self) {
        if self.wiki_selected_id.is_some() {
            return;
        }
        if !self.seed_input.trim().is_empty() {
            let q = self.seed_input.trim();
            if let Some(p) = self.wiki_pages.iter().find(|p| {
                p.title.eq_ignore_ascii_case(q) || p.slug.eq_ignore_ascii_case(q) || p.id == q
            }) {
                let id = p.id.clone();
                self.open_wiki_page_id(&id);
            }
        } else if let Some(p) = self
            .wiki_pages
            .iter()
            .find(|p| p.title.to_lowercase().contains("обзор") || p.slug.contains("overview"))
            .or_else(|| self.wiki_pages.first())
        {
            let id = p.id.clone();
            self.open_wiki_page_id(&id);
        }
    }

    fn open_wiki_page_id(&mut self, id: &str) {
        if self.wiki_dual_pane && self.wiki_focus == WikiPane::B {
            // Re-clicking the page already open in B must not refetch body+backlinks.
            if self.wiki_selected_id_b.as_deref() == Some(id) && self.wiki_article_b.is_some() {
                return;
            }
            self.open_wiki_page_in_b(id);
            return;
        }
        if self.pending_save.is_some() {
            self.wiki_error = Some(
                "Идёт сохранение. Дождитесь результата перед открытием другой страницы.".into(),
            );
            return;
        }
        // Same for pane A.
        if self.wiki_selected_id.as_deref() == Some(id) && self.wiki_article.is_some() {
            return;
        }
        if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("Есть несохранённые правки: сохраните или отмените их".into());
            return;
        }
        if let Some(prev) = self.wiki_selected_id.clone() {
            if prev != id {
                self.wiki_history.push(prev);
                if self.wiki_history.len() > 64 {
                    self.wiki_history.remove(0);
                }
            }
        }
        self.open_wiki_page_id_no_history(id);
    }

    fn open_wiki_page_id_no_history(&mut self, id: &str) {
        self.wiki_selected_id = Some(id.to_string());
        self.wiki_error = None;
        self.wiki_edit = None;
        self.wiki_save_note = None;
        let Some(source) = self.available_load_source() else {
            self.wiki_error = Some("Для статей вики требуется --http или --db".into());
            return;
        };
        let Some(meta) = self.wiki_pages.iter().find(|p| p.id == id).cloned() else {
            self.wiki_error = Some(format!("Страница {id} отсутствует в каталоге"));
            return;
        };
        // Keep the current article visible until the new body arrives.
        let seq = self.next_seq();
        self.pending_page_a = Some(seq);
        self.worker.send(WorkerCmd::OpenPage {
            seq,
            pane_b: false,
            push_history: false,
            meta: Some(meta),
            q: None,
            source,
        });
    }

    /// Load a page into the right (secondary) dual-pane column. No history / edit.
    fn open_wiki_page_in_b(&mut self, id: &str) {
        self.wiki_selected_id_b = Some(id.to_string());
        self.wiki_error_b = None;
        let Some(source) = self.available_load_source() else {
            self.wiki_error_b = Some("Для статей вики требуется --http или --db".into());
            return;
        };
        let Some(meta) = self.wiki_pages.iter().find(|p| p.id == id).cloned() else {
            self.wiki_error_b = Some(format!("Страница {id} отсутствует в каталоге"));
            return;
        };
        let seq = self.next_seq();
        self.pending_page_b = Some(seq);
        self.worker.send(WorkerCmd::OpenPage {
            seq,
            pane_b: true,
            push_history: false,
            meta: Some(meta),
            q: None,
            source,
        });
    }

    fn clear_wiki_pane_b(&mut self) {
        self.wiki_selected_id_b = None;
        self.wiki_article_b = None;
        self.wiki_error_b = None;
        self.pending_page_b = None;
    }

    fn reset_wiki_for_project(&mut self) {
        self.worker.cancel_wiki_reads();
        self.pending_catalog = None;
        self.pending_page_a = None;
        self.pending_page_b = None;
        self.pending_backlinks = None;
        self.wiki_pages.clear();
        self.wiki_selected_id = None;
        self.wiki_article = None;
        self.wiki_backlinks.clear();
        self.wiki_backlinks_error = None;
        self.wiki_history.clear();
        self.wiki_error = None;
        self.wiki_edit = None;
        self.wiki_save_note = None;
        self.wiki_open_after_catalog = None;
        self.clear_wiki_pane_b();
        self.wiki_loaded = false;
        if self.mode == ViewMode::Wiki {
            self.reload_wiki_catalog();
        }
    }

    /// Apply a worker PageOpened result to the target pane.
    fn apply_page_opened(
        &mut self,
        pane_b: bool,
        push_history: bool,
        q: Option<&str>,
        result: Result<DocumentBody, String>,
    ) {
        match result {
            Ok(body) => {
                // Unresolved-link fallback may open a page that is not in the
                // catalog yet; add a row so the sidebar stays consistent.
                if !self.wiki_pages.iter().any(|p| p.id == body.id) {
                    let slug = slug_from_wiki_uri(&body.uri);
                    self.wiki_pages.push(WikiPageMeta {
                        id: body.id.clone(),
                        uri: body.uri.clone(),
                        slug,
                        title: body.title.clone(),
                        kind: body.kind.clone(),
                        summary: None,
                        category: None,
                        revision: body.revision.unwrap_or(1),
                        etag: body.etag.clone(),
                        updated_at: body.updated_at.clone(),
                    });
                    sort_wiki_pages(&mut self.wiki_pages);
                }
                if pane_b {
                    self.wiki_selected_id_b = Some(body.id.clone());
                    self.wiki_article_b = Some(body);
                    self.wiki_error_b = None;
                } else {
                    let body_id = body.id.clone();
                    if push_history {
                        if let Some(prev) = self.wiki_selected_id.clone() {
                            if prev != body.id {
                                self.wiki_history.push(prev);
                                if self.wiki_history.len() > 64 {
                                    self.wiki_history.remove(0);
                                }
                            }
                        }
                    }
                    self.wiki_selected_id = Some(body.id.clone());
                    self.wiki_article = Some(body);
                    self.wiki_backlinks.clear();
                    self.wiki_backlinks_error = None;
                    self.wiki_error = None;
                    self.wiki_save_note = None;
                    self.refresh_backlinks(&body_id);
                }
            }
            Err(e) => {
                let msg = match q {
                    Some(q) => format!(
                        "Ссылка [[{q}]] не разрешена: страницы с таким названием/slug нет (создайте через write_wiki_page): {e}"
                    ),
                    None => e,
                };
                if pane_b {
                    self.wiki_article_b = None;
                    self.wiki_error_b = Some(msg);
                } else {
                    self.wiki_article = None;
                    self.wiki_backlinks.clear();
                    self.wiki_backlinks_error = None;
                    self.wiki_error = Some(msg);
                }
            }
        }
    }

    fn wiki_can_write(&self) -> bool {
        self.activity_base().is_some()
    }

    fn start_wiki_edit(&mut self) {
        let Some(art) = self.wiki_article.as_ref() else {
            return;
        };
        if !self.wiki_can_write() {
            self.wiki_error = Some(
                "Для редактирования нужен HTTP gateway; режим --db доступен только для чтения"
                    .into(),
            );
            return;
        }
        self.wiki_save_note = None;
        self.wiki_error = None;
        self.wiki_edit = Some(WikiEditBuffers::from_article(art));
    }

    fn cancel_wiki_edit(&mut self) {
        if self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед закрытием редактора.".into());
            return;
        }
        self.wiki_edit = None;
        self.wiki_error = None;
    }

    /// Discard edits and refetch the current revision (409 CAS conflict path).
    fn reload_wiki_page(&mut self) {
        if self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед обновлением страницы.".into());
            return;
        }
        let id = self
            .wiki_selected_id
            .clone()
            .or_else(|| self.wiki_article.as_ref().map(|a| a.id.clone()));
        self.cancel_wiki_edit();
        if let Some(id) = id {
            self.open_wiki_page_id_no_history(&id);
        }
    }

    /// Dispatch the save on the worker (EGUI_GRAPH_VIEW §2.5: no blocking IO on
    /// the UI thread). Edit buffers stay until `SavedPage` lands, so a 409/CAS
    /// conflict keeps the user's text; a repeated Save/Ctrl+S is ignored while
    /// a save is in flight.
    fn save_wiki_edit(&mut self) {
        if self.pending_save.is_some() {
            return;
        }
        let Some(edit) = self.wiki_edit.as_ref() else {
            return;
        };
        let Some(art) = self.wiki_article.as_ref() else {
            self.wiki_error = Some("Нет открытой страницы для сохранения".into());
            return;
        };
        let Some(source) = self.available_load_source() else {
            self.wiki_error = Some("Для сохранения вики требуется HTTP gateway".into());
            return;
        };
        if !matches!(&source, LoadSource::Http(_)) {
            self.wiki_error = Some(
                "Режим --db доступен только для чтения; переподключитесь через HTTP gateway".into(),
            );
            return;
        }
        let req = WikiPutRequest {
            id: art.id.clone(),
            slug: slug_from_wiki_uri(&art.uri),
            uri: Some(art.uri.clone()),
            title: edit.title.clone(),
            content: edit.content.clone(),
            if_match_revision: edit.base_revision,
            if_match_etag: edit.base_etag.clone(),
        };
        let seq = self.next_seq();
        self.pending_save = Some(seq);
        self.worker.send(WorkerCmd::SavePage { seq, req, source });
    }

    /// Apply a worker SavedPage result (success refreshes catalog + backlinks;
    /// a conflict error keeps the edit buffers and offers Reload in the edit view).
    fn apply_save_result(&mut self, result: Result<DocumentBody, String>) {
        match result {
            Ok(body) => {
                // Refresh catalog row for title/summary/revision.
                if let Some(meta) = self.wiki_pages.iter_mut().find(|p| p.id == body.id) {
                    meta.title = body.title.clone();
                    meta.revision = body.revision.unwrap_or(meta.revision);
                    meta.etag = body.etag.clone();
                    meta.updated_at = body.updated_at.clone();
                    if let Some(summary) = content_summary_line(&body.content) {
                        meta.summary = Some(summary);
                    }
                }
                sort_wiki_pages(&mut self.wiki_pages);
                self.refresh_backlinks(&body.id);
                let rev_note = body
                    .revision
                    .map(|r| format!(" · версия r{r}"))
                    .unwrap_or_default();
                self.wiki_save_note = Some(format!("Сохранено «{}»{rev_note}", body.title));
                self.wiki_article = Some(body);
                self.wiki_edit = None;
                self.wiki_error = None;
            }
            Err(e) => {
                self.wiki_error = Some(e);
                self.wiki_save_note = None;
            }
        }
    }

    fn refresh_backlinks(&mut self, document_id: &str) {
        let Some(source) = self.available_load_source() else {
            return;
        };
        let seq = self.next_seq();
        self.pending_backlinks = Some(seq);
        self.wiki_backlinks_error = None;
        self.worker.send(WorkerCmd::LoadBacklinks {
            seq,
            document_id: document_id.to_string(),
            source,
            project: nonempty(&self.filter_wing),
        });
    }

    fn wiki_known_keys(&self) -> (HashSet<String>, HashSet<String>) {
        let mut titles = HashSet::new();
        let mut slugs = HashSet::new();
        for p in &self.wiki_pages {
            titles.insert(p.title.clone());
            slugs.insert(p.slug.clone());
        }
        (titles, slugs)
    }

    fn wiki_go_back(&mut self) {
        if self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед переходом назад.".into());
            return;
        }
        if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("Есть несохранённые правки: сохраните или отмените их".into());
            return;
        }
        if let Some(prev) = self.wiki_history.pop() {
            self.wiki_focus = WikiPane::A;
            self.open_wiki_page_id_no_history(&prev);
        }
    }

    /// Resolve `[[link]]` text to a catalog page and open it (exact title/slug/id only).
    fn open_wiki_link(&mut self, link: &str) {
        let into_b = self.wiki_dual_pane && self.wiki_focus == WikiPane::B;
        if !into_b && self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед переходом по ссылке.".into());
            return;
        }
        if !into_b && self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("Есть несохранённые правки: сохраните или отмените их".into());
            return;
        }
        let q = link.trim();
        if q.is_empty() {
            return;
        }
        if let Some(p) = self.wiki_pages.iter().find(|p| {
            p.title == q
                || p.title.eq_ignore_ascii_case(q)
                || p.slug.eq_ignore_ascii_case(q)
                || p.id == q
                || p.uri == q
                || p.uri == format!("wiki://{q}")
        }) {
            let id = p.id.clone();
            self.open_wiki_page_id(&id);
            return;
        }
        if !self.filter_wing.trim().is_empty() {
            let msg = format!(
                "Ссылка [[{q}]] не найдена в проекте {}",
                self.filter_wing.trim()
            );
            if into_b {
                self.wiki_error_b = Some(msg);
            } else {
                self.wiki_error = Some(msg);
            }
            return;
        }
        // Exact wiki uri fallback (no fuzzy label pick) - fetched on the worker.
        let Some(source) = self.available_load_source() else {
            let msg = "Для wiki-ссылок требуется --http или --db".to_string();
            if into_b {
                self.wiki_error_b = Some(msg);
            } else {
                self.wiki_error = Some(msg);
            }
            return;
        };
        let seq = self.next_seq();
        if into_b {
            self.pending_page_b = Some(seq);
        } else {
            self.pending_page_a = Some(seq);
        }
        self.worker.send(WorkerCmd::OpenPage {
            seq,
            pane_b: into_b,
            push_history: !into_b,
            meta: None,
            q: Some(q.to_string()),
            source,
        });
    }

    fn selected_graph_wiki_target(&self) -> Option<GraphWikiTarget> {
        let selected = self.selected.as_deref()?;
        let node = self
            .ui_graph
            .as_ref()?
            .nodes
            .iter()
            .find(|node| node.id == selected)?;
        graph_wiki_target(
            node.document_id.as_deref(),
            node.uri.as_deref(),
            &self.wiki_pages,
        )
    }

    fn open_selected_graph_node_in_wiki(&mut self) {
        if self.available_load_source().is_none() {
            self.content_error = Some(
                "В snapshot-режиме доступна только топология; откройте HTTP или DB источник".into(),
            );
            return;
        }
        let Some(sel) = self.selected.clone() else {
            return;
        };
        let (doc_id, target) = {
            let Some(g) = self.ui_graph.as_ref() else {
                return;
            };
            let Some(node) = g.nodes.iter().find(|n| n.id == sel) else {
                return;
            };
            let target = graph_wiki_target(
                node.document_id.as_deref(),
                node.uri.as_deref(),
                &self.wiki_pages,
            );
            (node.document_id.clone(), target)
        };
        let Some(target) = target else {
            self.content_error = Some(
                "Выбранный узел не является wiki-страницей; используйте превью или корпус".into(),
            );
            return;
        };
        self.mode = ViewMode::Wiki;
        self.wiki_focus = WikiPane::A;
        match target {
            GraphWikiTarget::CatalogId(id) => self.open_wiki_page_id(&id),
            GraphWikiTarget::WikiUri(uri) => {
                if !self.filter_wing.trim().is_empty() {
                    if !self.wiki_loaded {
                        if let Some(id) = doc_id {
                            self.wiki_open_after_catalog = Some(id);
                            self.ensure_wiki_loaded();
                        } else {
                            self.wiki_error = Some(format!(
                                "Нельзя открыть {uri} в проекте {}, пока страница не появится в его wiki-каталоге",
                                self.filter_wing.trim()
                            ));
                        }
                    } else {
                        self.wiki_error = Some(format!(
                            "Wiki-страницы {uri} нет в проекте {}",
                            self.filter_wing.trim()
                        ));
                    }
                } else {
                    self.open_wiki_link(&uri);
                }
            }
        }
    }

    fn apply_seed_from_input(&mut self) {
        self.seed_error = None;
        let seed = self.seed_input.trim();
        if seed.is_empty() {
            self.seed_id = None;
            self.rebuild_ui_graph();
            return;
        }
        let Some(full) = self.full_view.as_ref() else {
            self.seed_error = Some("Граф не загружен".into());
            return;
        };
        match resolve_seed(full, seed) {
            Ok(id) => {
                self.seed_id = Some(id);
                self.rebuild_ui_graph();
            }
            Err(e) => {
                self.seed_id = None;
                self.seed_error = Some(e);
                self.local_view = None;
                self.local_truncated = false;
                self.ui_graph = None;
                self.layout_ready = false;
            }
        }
    }

    /// Load full document body for the current selection (wiki or raw).
    fn load_content_for_selected(&mut self) {
        self.content_error = None;
        let Some(sel) = self.selected.clone() else {
            self.content_error = Some("Ничего не выбрано".into());
            return;
        };
        let Some(g) = self.ui_graph.as_ref() else {
            self.content_error = Some("Представление графа недоступно".into());
            return;
        };
        let Some(node) = g.nodes.iter().find(|n| n.id == sel) else {
            self.content_error = Some("Выбранного узла нет в текущем представлении".into());
            return;
        };
        if node.document_id.is_none() && node.uri.is_none() {
            self.content_error = Some(format!(
                "У узла «{}» нет документа ({} / unresolved stub?)",
                node.label, node.kind
            ));
            return;
        }
        let (doc_id, uri, label) = (
            node.document_id.clone(),
            node.uri.clone(),
            node.label.clone(),
        );

        let Some(source) = self.available_load_source() else {
            self.content_error = Some(
                "В snapshot-режиме нет содержимого документов; используйте --http или --db".into(),
            );
            return;
        };

        // Keep previous content visible while the worker fetches.
        let seq = self.next_seq();
        self.pending_content = Some(seq);
        self.worker.send(WorkerCmd::ReadContent {
            seq,
            node_id: sel,
            doc_id,
            uri,
            label,
            source,
        });
    }

    fn rebuild_ui_graph(&mut self) {
        self.expand_note = None;
        self.expanded_dirty = false;
        self.worker.cancel_expand_read();
        // Any in-flight Expand merges old topology into the new local view:
        // invalidate it so the late answer is dropped by seq.
        self.pending_expand = None;
        let Some(full) = self.full_view.as_ref() else {
            self.local_view = None;
            self.local_truncated = false;
            self.ui_graph = None;
            self.layout_ready = false;
            return;
        };

        let Some(seed) = self.seed_id.as_deref() else {
            if self.graph_lens == GraphLens::Neighborhood {
                self.local_view = None;
                self.local_truncated = false;
                self.ui_graph = None;
                self.positions.clear();
                self.layout_ready = false;
                self.need_fit = true;
                return;
            }
            let pkb_rels_only = !matches!(
                self.graph_lens,
                GraphLens::Architecture | GraphLens::Provenance
            );
            let opts = AdaptOptions {
                lens: self.graph_lens,
                seed_id: None,
                show_tags: self.show_tags,
                show_stubs: self.show_stubs,
                pkb_rels_only,
                wing: nonempty(&self.filter_wing),
                room: nonempty(&self.filter_room),
                show_all_nodes: true,
            };
            let overview = adapt(full, &opts);
            overview_grid_place(&overview, &mut self.positions);
            self.local_view = None;
            self.local_truncated = false;
            self.ui_graph = Some(overview);
            self.need_fit = true;
            self.layout_ready = true;
            return;
        };

        let local = local_neighbors_bounded(full, seed, self.depth, self.max_nodes as usize);
        self.local_truncated = local.capped;
        self.apply_local_topology(local.view, true);
    }

    /// Adapt `local`, update layout. `reset_layout` re-runs RadialLocal; expand uses false.
    fn apply_local_topology(&mut self, local: GraphView, reset_layout: bool) {
        let seed = self.seed_id.clone();
        let pkb_rels_only = !matches!(
            self.graph_lens,
            GraphLens::Architecture | GraphLens::Provenance
        );
        let opts = AdaptOptions {
            lens: self.graph_lens,
            seed_id: seed.clone(),
            show_tags: self.show_tags,
            show_stubs: self.show_stubs,
            pkb_rels_only,
            wing: nonempty(&self.filter_wing),
            room: nonempty(&self.filter_room),
            show_all_nodes: false,
        };
        let ui_graph = adapt(&local, &opts);
        let topo = topology_generation(&local);
        self.local_view = Some(local);

        if ui_graph.nodes.is_empty() {
            self.ui_graph = Some(ui_graph);
            self.layout_ready = false;
            return;
        }

        if reset_layout || self.positions.is_empty() {
            radial_place(&ui_graph, seed.as_deref(), &mut self.positions);
            self.need_fit = true;
        } else {
            // Expand or filter: keep existing positions; place newcomers near neighbors.
            place_missing_near_neighbors(&ui_graph, &mut self.positions);
        }
        self.last_topo = topo;
        self.ui_graph = Some(ui_graph);
        self.layout_ready = true;
    }

    /// Expand neighbors of the selection: one hop (depth+1 from that node), merge
    /// into the local view under `max_nodes`. Runs on the worker (EGUI_GRAPH_VIEW
    /// §5.1 / §6.1); does not reseed the local view.
    fn expand_selected(&mut self) {
        let Some(sel) = self.selected.clone() else {
            return;
        };
        self.expand_note = None;
        self.seed_error = None;

        // No seed yet: selected becomes seed at depth 1, then paint (no IO).
        if self.seed_id.is_none() {
            self.seed_id = Some(sel.clone());
            self.seed_input = sel;
            self.depth = self.depth.max(1);
            self.rebuild_ui_graph();
            return;
        }

        let max_n = self.max_nodes as usize;
        let current = match self.local_view.clone() {
            Some(v) if !v.nodes.is_empty() => v,
            _ => {
                // Seed set but local empty: rebuild first, then expand if still selected.
                self.rebuild_ui_graph();
                match self.local_view.clone() {
                    Some(v) => v,
                    None => return,
                }
            }
        };

        if current.nodes.len() >= max_n {
            self.expand_note = Some(format!(
                "Раскрытие недоступно: уже достигнут max_nodes ({max_n})"
            ));
            return;
        }

        let db_path = match self.open.source.clone() {
            Some(CliSource::Db(path)) => Some(path),
            _ => None,
        };
        let http_base = match self.source.as_ref() {
            Some(GraphSourceKind::HttpService { base }) => Some(base.clone()),
            _ => None,
        };
        let full = self.full_view.clone();
        if http_base.is_none() && db_path.is_none() && full.is_none() {
            self.expand_note = Some("Для раскрытия нужен загруженный граф".into());
            return;
        }

        // Status depth: at least selected.depth + 1 (one hop outward), capped at 3.
        if let Some(g) = &self.ui_graph {
            if let Some(n) = g.nodes.iter().find(|n| n.id == sel) {
                self.depth = self.depth.max((n.depth + 1).min(3));
            }
        } else {
            self.depth = self.depth.clamp(1, 3);
        }

        let seq = self.next_seq();
        self.pending_expand = Some(seq);
        self.worker.send(WorkerCmd::ExpandNeighbors {
            seq,
            selected: sel,
            current,
            full,
            db_path,
            http_base,
            project: nonempty(&self.filter_wing),
            include_tags: self.show_tags,
            max_nodes: self.max_nodes,
        });
    }

    fn empty_kind(&self) -> Option<(EmptyKind, Option<String>)> {
        if let Some(err) = &self.load_error {
            return Some((EmptyKind::LoadError, Some(err.clone())));
        }
        if self.open.source.is_none() && self.full_view.is_none() {
            return Some((EmptyKind::NoSource, None));
        }
        if let Some(full) = &self.full_view {
            if full.nodes.is_empty() {
                return Some((EmptyKind::EmptyGraph, None));
            }
            if self.seed_id.is_none() && self.seed_error.is_some() {
                return Some((EmptyKind::SeedNotFound, self.seed_error.clone()));
            }
            if self.seed_id.is_none() && self.graph_lens == GraphLens::Neighborhood {
                return Some((EmptyKind::MissingSeed, None));
            }
            if let Some(g) = &self.ui_graph {
                if g.nodes.is_empty() {
                    return Some((EmptyKind::FiltersEmpty, None));
                }
            }
        }
        None
    }

    /// Topology stats for empty banners when a source is loaded.
    fn empty_stats(&self) -> Option<EmptyGraphStats> {
        let full = self.full_view.as_ref()?;
        Some(EmptyGraphStats::from_view(
            full,
            self.raw_node_count,
            self.raw_truncated,
        ))
    }

    fn status_banner(&self) -> Option<String> {
        if let Some(e) = &self.seed_error {
            return Some(e.clone());
        }
        if let Some(e) = &self.expand_note {
            return Some(e.clone());
        }
        if self.local_truncated {
            return Some(format!(
                "Локальное окружение достигло max_nodes ({}); часть достижимых узлов скрыта",
                self.max_nodes
            ));
        }
        if let Some(note) = self.ui_graph.as_ref().and_then(|g| g.note.clone()) {
            return Some(note);
        }
        if self.raw_truncated {
            return Some(format!(
                "Показан локальный вид; исходный снимок содержал {} узлов (жёсткий лимит {})",
                self.raw_node_count, UI_HARD_MAX_NODES
            ));
        }
        None
    }

    /// Drain worker results; stale seqs are ignored (a newer job superseded them).
    fn drain_worker_events(&mut self) {
        while let Ok(evt) = self.worker.rx.try_recv() {
            match evt {
                WorkerEvt::GraphLoaded { seq, result } => {
                    if self.pending_graph != Some(seq) {
                        continue;
                    }
                    self.pending_graph = None;
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
                    if self.pending_project_catalog != Some(seq) {
                        continue;
                    }
                    self.pending_project_catalog = None;
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
                    if self.pending_project_home != Some(seq) {
                        continue;
                    }
                    self.pending_project_home = None;
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
                    if self.pending_library != Some(seq) {
                        continue;
                    }
                    self.pending_library = None;
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
                                self.pending_library_document = None;
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
                    if self.pending_library_document != Some(seq) {
                        continue;
                    }
                    self.pending_library_document = None;
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
                    if self.pending_search != Some(seq) {
                        continue;
                    }
                    self.pending_search = None;
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
                    if self.pending_operations != Some(seq) {
                        continue;
                    }
                    self.pending_operations = None;
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
                    if self.pending_jobs != Some(seq) {
                        continue;
                    }
                    self.pending_jobs = None;
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
                    if self.pending_operation_action != Some(seq) {
                        continue;
                    }
                    self.pending_operation_action = None;
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
                    if self.pending_operation_action != Some(seq) {
                        continue;
                    }
                    self.pending_operation_action = None;
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
                    if self.pending_revisions != Some(seq) {
                        continue;
                    }
                    self.pending_revisions = None;
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
                    if self.pending_revision_snapshot != Some(seq) {
                        continue;
                    }
                    self.pending_revision_snapshot = None;
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
                    if self.pending_revision_diff != Some(seq) {
                        continue;
                    }
                    self.pending_revision_diff = None;
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
                    if self.pending_revision_restore != Some(seq) {
                        continue;
                    }
                    self.pending_revision_restore = None;
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
                    if self.pending_catalog != Some(seq) {
                        continue;
                    }
                    self.pending_catalog = None;
                    self.wiki_loaded = true;
                    match result {
                        Ok(mut pages) => {
                            sort_wiki_pages(&mut pages);
                            self.wiki_pages = pages;
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
                    let pending = if pane_b {
                        &mut self.pending_page_b
                    } else {
                        &mut self.pending_page_a
                    };
                    if *pending != Some(seq) {
                        continue;
                    }
                    *pending = None;
                    self.apply_page_opened(pane_b, push_history, q.as_deref(), result);
                }
                WorkerEvt::Backlinks {
                    seq,
                    document_id,
                    result,
                } => {
                    if self.pending_backlinks != Some(seq) {
                        continue;
                    }
                    self.pending_backlinks = None;
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
                    if self.pending_content != Some(seq) {
                        continue;
                    }
                    self.pending_content = None;
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
                    if self.pending_expand != Some(seq) {
                        continue;
                    }
                    self.pending_expand = None;
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
                    if self.pending_save != Some(seq) {
                        continue;
                    }
                    self.pending_save = None;
                    self.apply_save_result(result);
                }
                WorkerEvt::Activity { seq, result } => {
                    if self.pending_activity != Some(seq) {
                        continue;
                    }
                    self.pending_activity = None;
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

    /// Global hotkeys. Plain keys are ignored while a text field has focus;
    /// Ctrl+S (save) works even while the content editor is focused.
    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        let (esc, save, find, global_search, back) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::F),
                i.modifiers.command && i.key_pressed(egui::Key::K),
                i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft),
            )
        });
        if save
            && self.mode == ViewMode::Wiki
            && self.wiki_edit.is_some()
            && self.wiki_can_write()
            && self.pending_save.is_none()
        {
            self.save_wiki_edit();
            return;
        }
        if global_search && self.activity_base().is_some() {
            self.activate_shell_route(ShellRoute::Search);
            ctx.memory_mut(|memory| {
                memory.request_focus(egui::Id::new("native_search_query"));
            });
            return;
        }
        if esc && self.mode == ViewMode::Wiki && self.wiki_edit.is_some() {
            self.cancel_wiki_edit();
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if esc && self.mode == ViewMode::Graph && self.selected.is_some() {
            self.selected = None;
            self.content = None;
            self.content_error = None;
            self.pending_content = None;
        }
        if find && self.mode == ViewMode::Wiki {
            ctx.memory_mut(|m| m.request_focus(wiki_filter_id()));
        }
        if back && self.mode == ViewMode::Wiki {
            self.wiki_go_back();
        }
    }

    fn current_close_risk(&self) -> Option<CloseRisk> {
        close_risk(
            self.wiki_edit.as_ref().is_some_and(|edit| edit.dirty),
            self.pending_save.is_some(),
        )
    }

    fn intercept_native_close(&mut self, ctx: &egui::Context) {
        if self.force_close || !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        if self.current_close_risk().is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_confirmation_open = true;
        }
    }

    fn draw_close_confirmation(&mut self, ctx: &egui::Context) {
        if !self.close_confirmation_open {
            return;
        }
        let Some(risk) = self.current_close_risk() else {
            // The requested save completed successfully while the close dialog
            // was waiting. Honor the original close request automatically.
            self.close_confirmation_open = false;
            self.force_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };

        #[derive(Clone, Copy)]
        enum Choice {
            None,
            KeepOpen,
            CloseAnyway,
        }
        let choice = egui::Modal::new(egui::Id::new("native_close_confirmation"))
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                match risk {
                    CloseRisk::UnsavedWikiEdits => {
                        ui.heading("Несохранённые правки вики");
                        ui.label("При закрытии изменения названия и текста будут потеряны.");
                    }
                    CloseRisk::WikiSaveInFlight => {
                        ui.heading("Сохранение вики ещё выполняется");
                        ui.label("Оставьте приложение открытым до ответа gateway. После успешного сохранения оно закроется автоматически.");
                    }
                }
                ui.add_space(12.0);
                let mut choice = Choice::None;
                ui.horizontal(|ui| {
                    let keep_label = match risk {
                        CloseRisk::UnsavedWikiEdits => "Продолжить редактирование",
                        CloseRisk::WikiSaveInFlight => "Оставить открытым",
                    };
                    if ui.button(keep_label).clicked() {
                        choice = Choice::KeepOpen;
                    }
                    if ui
                        .button(match risk {
                            CloseRisk::UnsavedWikiEdits => "Закрыть без сохранения",
                            CloseRisk::WikiSaveInFlight => "Всё равно закрыть",
                        })
                        .clicked()
                    {
                        choice = Choice::CloseAnyway;
                    }
                });
                choice
            })
            .inner;
        match choice {
            Choice::None => {}
            Choice::KeepOpen => self.close_confirmation_open = false,
            Choice::CloseAnyway => {
                self.close_confirmation_open = false;
                self.force_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

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
                                    ui.label(&event.kind);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wiki_page(id: &str, uri: &str) -> WikiPageMeta {
        WikiPageMeta {
            id: id.into(),
            uri: uri.into(),
            slug: "page".into(),
            title: "Page".into(),
            kind: "page".into(),
            summary: None,
            category: None,
            revision: 1,
            etag: None,
            updated_at: None,
        }
    }

    #[test]
    fn configured_http_remains_available_before_graph_load() {
        let configured = CliSource::Http("http://remote.example:7432".into());
        assert_eq!(
            http_base_for_sources(None, Some(&configured)).as_deref(),
            Some("http://remote.example:7432")
        );
        assert!(http_base_for_sources(None, Some(&CliSource::Db("db.duckdb".into()))).is_none());
    }

    #[test]
    fn native_close_risk_prefers_in_flight_save_over_dirty_draft() {
        assert_eq!(close_risk(false, false), None);
        assert_eq!(close_risk(true, false), Some(CloseRisk::UnsavedWikiEdits));
        assert_eq!(close_risk(false, true), Some(CloseRisk::WikiSaveInFlight));
        assert_eq!(close_risk(true, true), Some(CloseRisk::WikiSaveInFlight));
    }

    #[test]
    fn graph_open_page_accepts_only_proven_wiki_targets() {
        let pages = vec![wiki_page("wiki-1", "wiki://one")];
        assert_eq!(
            graph_wiki_target(Some("wiki-1"), Some("file:///wrong.md"), &pages),
            Some(GraphWikiTarget::CatalogId("wiki-1".into()))
        );
        assert_eq!(
            graph_wiki_target(Some("unknown"), Some("wiki://missing"), &pages),
            Some(GraphWikiTarget::WikiUri("wiki://missing".into()))
        );
        assert_eq!(
            graph_wiki_target(Some("doc-1"), Some("file:///doc.md"), &pages),
            None
        );
        assert_eq!(graph_wiki_target(None, None, &pages), None);
    }

    #[test]
    fn enter_prefers_exact_seed_over_first_substring_suggestion() {
        let exact = rag_mcp::GraphNode {
            id: "z-exact".into(),
            kind: "document".into(),
            label: "Alpha".into(),
            document_id: Some("doc-alpha".into()),
            uri: None,
            resolved: true,
            metadata_json: "{}".into(),
        };
        let misleading = rag_mcp::GraphNode {
            id: "a-substring".into(),
            label: "Alpha appendix".into(),
            ..exact.clone()
        };
        let view = GraphView {
            nodes: vec![misleading, exact],
            edges: Vec::new(),
        };

        assert_eq!(
            seed_target_on_enter(Some(&view), "Alpha", Some("a-substring")).as_deref(),
            Some("z-exact")
        );
        assert_eq!(
            seed_target_on_enter(Some(&view), "unknown", Some("a-substring")).as_deref(),
            Some("a-substring")
        );
    }

    #[test]
    fn project_switch_clears_revision_workspace_and_leaves_history_mode() {
        let mut revision = RevisionWorkspace {
            document_id: Some("project-a-doc".into()),
            document_title: "Project A".into(),
            document_uri: "doc://project-a".into(),
            document_layer: "wiki".into(),
            head: Some(9),
            items: vec![RevisionItem {
                document_id: "project-a-doc".into(),
                uri: "doc://project-a".into(),
                title: "Project A".into(),
                wing: Some("project-a".into()),
                room: None,
                layer: "wiki".into(),
                kind: "page".into(),
                status: "superseded".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                superseded_at: "2026-01-02T00:00:00Z".into(),
                revision: 8,
                content_chars: 4,
                content_lines: 1,
            }],
            total: 9,
            next_cursor: Some("next".into()),
            selected: Some(8),
            snapshot: Some(DocumentBody {
                id: "project-a-doc".into(),
                uri: "doc://project-a".into(),
                title: "Project A".into(),
                layer: "wiki".into(),
                kind: "page".into(),
                content: "old".into(),
                content_hash: None,
                updated_at: None,
                revision: Some(8),
                etag: None,
            }),
            snapshot_error: Some("snapshot error".into()),
            diff: Some(RevisionDiff {
                document_id: "project-a-doc".into(),
                from_revision: 8,
                to_revision: 9,
                title_changed: false,
                metadata_changed: false,
                placement_changed: false,
                added_lines: 1,
                removed_lines: 1,
                changes: Vec::new(),
                truncated: false,
            }),
            diff_error: Some("diff error".into()),
            restore_confirm: Some(8),
            restore_result: Some(RestoreRevisionResult {
                document_id: "project-a-doc".into(),
                restored_from_revision: 8,
                revision: 10,
                etag: "etag".into(),
                chunk_count: 1,
                node_id: "node".into(),
                edge_count: 0,
            }),
            error: Some("timeline error".into()),
        };

        revision.clear();

        assert!(revision.document_id.is_none());
        assert!(revision.document_title.is_empty());
        assert!(revision.items.is_empty());
        assert_eq!(revision.total, 0);
        assert!(revision.next_cursor.is_none());
        assert!(revision.selected.is_none());
        assert!(revision.snapshot.is_none());
        assert!(revision.snapshot_error.is_none());
        assert!(revision.diff.is_none());
        assert!(revision.diff_error.is_none());
        assert!(revision.restore_confirm.is_none());
        assert!(revision.restore_result.is_none());
        assert!(revision.error.is_none());
        assert_eq!(
            mode_after_project_switch(ViewMode::Revisions),
            ViewMode::Library
        );
        assert_eq!(mode_after_project_switch(ViewMode::Graph), ViewMode::Graph);
    }

    #[test]
    fn project_scoped_responses_must_match_current_selection() {
        assert!(project_scope_matches(Some("alpha"), "alpha"));
        assert!(project_scope_matches(None, ""));
        assert!(!project_scope_matches(Some("alpha"), "beta"));
        assert!(!project_scope_matches(Some("alpha"), ""));
        assert!(!project_scope_matches(None, "alpha"));
    }

    #[test]
    fn project_switch_during_restore_keeps_previous_selection() {
        let mut selected = "project-b".to_string();
        let restore_in_flight = has_noncancellable_mutation(false, false, true);

        let changed = reconcile_project_change(&mut selected, "project-a", restore_in_flight);

        assert!(!changed);
        assert_eq!(selected, "project-a");

        let changed = reconcile_project_change(&mut selected, "project-a", false);
        assert!(!changed);
        selected = "project-b".to_string();
        assert!(reconcile_project_change(&mut selected, "project-a", false));
    }

    #[test]
    fn search_to_library_clears_incompatible_catalog_filters() {
        let mut request = LibraryRequest {
            q: "old".into(),
            wing: "old-project".into(),
            room: "private".into(),
            layer: "wiki".into(),
            kind: "page".into(),
            status: "archived".into(),
            include_archived: true,
            limit: 50,
            cursor: Some("next".into()),
        };
        prepare_library_request_from_search(
            &mut request,
            "alpha",
            "Result title",
            "file:///alpha/result.md",
            true,
        );
        assert_eq!(request.q, "file:///alpha/result.md");
        assert_eq!(request.wing, "alpha");
        assert!(request.room.is_empty());
        assert!(request.layer.is_empty());
        assert!(request.kind.is_empty());
        assert!(request.status.is_empty());
        assert!(request.include_archived);
        assert!(request.cursor.is_none());
    }

    #[test]
    fn only_mutating_operations_enter_the_single_flight_slot() {
        assert!(is_operations_mutation(&OperationsAction::Checkpoint));
        assert!(is_operations_mutation(&OperationsAction::CancelJob(
            "job".into()
        )));
        assert!(!is_operations_mutation(&OperationsAction::RefreshJobs));
        assert!(!is_operations_mutation(&OperationsAction::None));
    }
}
