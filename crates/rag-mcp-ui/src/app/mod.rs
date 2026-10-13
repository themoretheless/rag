//! GraphApp: eframe::App - native project, retrieval, document, wiki, and graph console.
//!
//! All blocking IO (HTTP / DuckDB) runs on the worker thread (`crate::worker`);
//! this file only dispatches [`WorkerCmd`] and applies [`WorkerEvt`] in `update()`.
//! Each job carries a `seq`; late answers whose seq no longer matches the pending
//! slot are dropped (race protection per EGUI_GRAPH_VIEW §2.5 / §8.3).

use egui::Vec2;
use rag_mcp_core::{GraphView, UI_GRAPH_EXPORT_MAX_NODES};

use crate::adapter::{adapt, topology_generation, AdaptOptions, GraphLens, UiGraph};
use crate::layout::{overview_grid_place, place_missing_near_neighbors, radial_place, PosCache};
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
use crate::ui::sync::draw_sync_workspace;
use crate::ui::theme;
use crate::ui::wiki::{
    can_cancel_edit, content_summary_line, draw_wiki_edit_view, draw_wiki_info_panel,
    draw_wiki_read_view, draw_wiki_sidebar, slug_from_wiki_uri, wiki_filter_id, WikiEditBuffers,
    WikiKnownKeys, WikiReadContext, WikiSidebarIndex,
};
use crate::worker::{LoadSource, WorkerCmd, WorkerEvt, WorkerHandle};

mod events;
mod frame;
mod graph;
mod panels;
mod revisions;
mod wiki;
mod workspace;

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
    Sync,
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

/// Cached seed-picker matches: `(node id, label)` pairs for one query over
/// one `full_view` revision.
pub(super) struct SeedSuggestions {
    pub(super) rev: u64,
    pub(super) query: String,
    pub(super) items: Vec<(String, String)>,
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
    /// Bumped whenever `full_view` is replaced; keys the seed-picker cache.
    full_view_rev: u64,
    /// Seed picker suggestions for (`full_view_rev`, lowercased query): the
    /// scan over the loaded graph (up to `UI_GRAPH_EXPORT_MAX_NODES`) runs
    /// once per query change, not every frame while the field has focus.
    seed_suggestions_cache: Option<SeedSuggestions>,
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
    /// Bumped on every `wiki_pages` change; keys the link-resolution cache.
    wiki_pages_rev: u64,
    /// ASCII-lowercased titles and slugs for `wiki_pages_rev`, built once per
    /// catalog change instead of on every frame.
    wiki_known_keys_cache: Option<(u64, std::rc::Rc<WikiKnownKeys>)>,
    /// Sidebar index for `wiki_pages_rev` (search text, category groups).
    wiki_sidebar_index_cache: Option<(u64, std::rc::Rc<WikiSidebarIndex>)>,
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
            full_view_rev: 0,
            seed_suggestions_cache: None,
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
            wiki_pages_rev: 0,
            wiki_known_keys_cache: None,
            wiki_sidebar_index_cache: None,
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
            ViewMode::Sync => ShellRoute::Sync,
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
            ShellRoute::Sync => {
                self.mode = ViewMode::Sync;
                self.reload_operations();
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
            | ViewMode::Sync
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
        self.full_view_rev += 1;
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
        self.full_view_rev += 1;
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
            ViewMode::Sync => self.reload_operations(),
            ViewMode::Models => self.reload_operations(),
            ViewMode::Evaluation => self.refresh_activity(),
            ViewMode::Search | ViewMode::Revisions | ViewMode::Graph => {}
        }
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
        let exact = rag_mcp_core::GraphNode {
            id: "z-exact".into(),
            kind: "document".into(),
            label: "Alpha".into(),
            document_id: Some("doc-alpha".into()),
            uri: None,
            resolved: true,
            metadata_json: "{}".into(),
        };
        let misleading = rag_mcp_core::GraphNode {
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
