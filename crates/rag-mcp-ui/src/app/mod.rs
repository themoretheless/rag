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
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cli::{CliSource, OpenArgs};

use crate::dto::{
    sort_wiki_pages, ActivityEvent, DocumentBody, GatewayHealth, GraphSourceKind, LoadedGraph,
    WikiPageMeta, WikiPutRequest,
};

use self::pending::PendingSlots;
use self::state::{
    ActivityState, GraphViewState, HomeState, LibraryState, OpsState, ProjectScope, SearchState,
    WikiState,
};
use crate::operations::{JobSnapshot, MaintenanceResult, OperationsSnapshot};
use crate::product::{LibraryItem, LibraryPage, LibraryRequest, ProjectHome};
use crate::revisions::{RestoreRevisionResult, RevisionDiff, RevisionItem};
use crate::search::{SearchRequest, SearchResults};
use crate::source::{DocumentSource, SourceRegistry};
use crate::topology::{
    local_neighbors_bounded, resolve_exact_seed, resolve_seed, UI_HARD_MAX_NODES,
};
use crate::ui::canvas::draw_canvas;
use crate::ui::detail::{draw_detail, DetailAction, DetailContentState};
use crate::ui::empty::{
    draw_empty_banner, draw_no_source, EmptyGraphStats, EmptyKind, NoSourceAction,
};
use crate::ui::home::{draw_project_home, HomeAction, ProjectHomeView};
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
use crate::ui::status::{draw_status, StatusView};
use crate::ui::sync::draw_sync_workspace;
use crate::ui::theme;
use crate::ui::wiki::{
    can_cancel_edit, content_summary_line, draw_wiki_edit_view, draw_wiki_info_panel,
    draw_wiki_read_view, draw_wiki_sidebar, slug_from_wiki_uri, wiki_filter_id, WikiEditBuffers,
    WikiKnownKeys, WikiReadContext, WikiSidebarIndex,
};
use crate::worker::{WorkerCmd, WorkerEvt, WorkerHandle};

mod events;
mod frame;
mod graph;
mod panels;
mod pending;
mod revisions;
mod state;
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
enum WikiFocus {
    #[default]
    A,
    B,
}

/// What a selection change does to a document request still in flight for the
/// previously selected node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InFlightContent {
    /// Drop it: the detail panel stops showing "loading" at once.
    Cancel,
    /// Leave the slot alone; the answer is still discarded unless that node is
    /// selected again by the time it arrives.
    Keep,
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

pub(super) struct GraphWikiTargetMemo {
    wiki_pages_rev: u64,
    document_id: Option<String>,
    uri: Option<String>,
    target: Option<GraphWikiTarget>,
}

pub struct GraphApp {
    open: OpenArgs,
    worker: WorkerHandle,
    /// Backends behind `open.source` / the loaded graph (shares the `--db` store).
    sources: SourceRegistry,
    /// Monotonic job counter; every pending slot stores the seq it waits for.
    seq: u64,
    pending: PendingSlots,
    /// HTTP base URL editable on the no-source start screen.
    connect_url: String,
    close_confirmation_open: bool,
    force_close: bool,

    /// Where the loaded topology came from (status line, capability checks).
    source: Option<GraphSourceKind>,
    mode: ViewMode,
    scope: ProjectScope,
    graph: GraphViewState,
    home: HomeState,
    library: LibraryState,
    search: SearchState,
    ops: OpsState,
    activity: ActivityState,
    revision: RevisionWorkspace,
    wiki: WikiState,
}

impl GraphApp {
    /// Build the app from CLI open args, installing the theme and clamping depth/node limits.
    pub fn new(cc: &eframe::CreationContext<'_>, open: OpenArgs) -> Self {
        theme::install(&cc.egui_ctx);
        let connect_url = match open.source.as_ref() {
            Some(CliSource::Http(base)) => base.clone(),
            _ => "http://127.0.0.1:7432".into(),
        };
        let graph = GraphViewState::new(&open);
        let mut app = Self {
            open,
            worker: crate::worker::spawn(cc.egui_ctx.clone()),
            sources: SourceRegistry::default(),
            seq: 0,
            pending: PendingSlots::default(),
            connect_url,
            close_confirmation_open: false,
            force_close: false,
            source: None,
            mode: ViewMode::Home,
            scope: ProjectScope::default(),
            graph,
            home: HomeState::default(),
            library: LibraryState::default(),
            search: SearchState::default(),
            ops: OpsState::default(),
            activity: ActivityState::default(),
            revision: RevisionWorkspace::default(),
            wiki: WikiState::default(),
        };
        app.dispatch_graph_load();
        app
    }

    /// The single place the graph selection changes. The previous node's
    /// document (and any error about it) never carries over; `in_flight` says
    /// whether a document request still running for it is dropped as well.
    fn select_node(&mut self, id: Option<String>, in_flight: InFlightContent) {
        self.graph.selected = id;
        self.graph.content = None;
        self.graph.content_error = None;
        if in_flight == InFlightContent::Cancel {
            self.pending.content.clear();
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn any_pending(&self) -> bool {
        self.pending.any()
    }

    fn noncancellable_mutation_in_flight(&self) -> bool {
        has_noncancellable_mutation(
            self.pending.save.is_some(),
            self.pending.operation_action.is_some(),
            self.pending.revision_restore.is_some(),
        )
    }

    fn mutation_status_label(&self) -> Option<&'static str> {
        if self.pending.save.is_some() {
            Some("сохранение…")
        } else if self.pending.revision_restore.is_some() {
            Some("восстановление…")
        } else if self.pending.operation_action.is_some() {
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
                if self.ops.snapshot.is_none() && self.pending.operations.is_none() {
                    self.reload_operations();
                }
                if self.pending.activity.is_none() {
                    self.refresh_activity();
                }
            }
            ShellRoute::Corpus => {
                self.mode = ViewMode::Library;
                self.ensure_library_loaded();
            }
            ShellRoute::Search => {
                self.mode = ViewMode::Search;
                self.search.request.wing = nonempty(&self.scope.wing);
            }
            ShellRoute::Graph => self.mode = ViewMode::Graph,
            ShellRoute::Wiki => {
                self.mode = ViewMode::Wiki;
                self.ensure_wiki_loaded();
            }
            ShellRoute::Agents => {
                self.mode = ViewMode::Activity;
                self.ops.tab = OperationsTab::Activity;
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
                if self.ops.snapshot.is_none() && self.pending.operations.is_none() {
                    self.reload_operations();
                }
            }
        }
    }

    fn shell_health(&self) -> (Option<bool>, String) {
        if self.activity_base().is_none() {
            if self.graph.load_error.is_some() && self.shell_read_only_source().is_some() {
                return (None, "источник только для чтения не загружен".into());
            }
            return (
                None,
                self.shell_read_only_source()
                    .unwrap_or("gateway · офлайн")
                    .into(),
            );
        }
        if self.pending.operations.is_some() {
            return (None, "gateway · обновление состояния…".into());
        }
        if self.ops.snapshot_stale {
            return (None, "gateway · состояние устарело".into());
        }
        if self.ops.error.is_some() {
            return (None, "gateway · требуется внимание".into());
        }
        if let Some(snapshot) = &self.ops.snapshot {
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
        if let Some(health) = &self.ops.health {
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

    /// Gateway base URL for a product request. Without a gateway, hands
    /// "<purpose> требуется HTTP gateway" to `report` (which stores it in the
    /// caller's error slot) and returns `None`.
    fn gateway(&mut self, purpose: &str, report: impl FnOnce(&mut Self, String)) -> Option<String> {
        let base = self.activity_base();
        if base.is_none() {
            report(self, format!("{purpose} требуется HTTP gateway"));
        }
        base
    }

    fn topology_query_requires_reload(&self) -> bool {
        self.activity_base().is_some()
            || matches!(self.source, Some(GraphSourceKind::LiveStore { .. }))
            || matches!(self.open.source, Some(CliSource::Db(_)))
    }

    fn available_load_source(&self) -> Option<Arc<dyn DocumentSource>> {
        self.source
            .as_ref()
            .and_then(|loaded| self.sources.for_loaded(loaded))
            .or_else(|| match self.open.source.as_ref() {
                // A configured HTTP gateway is sufficient for document/wiki
                // APIs even before the independent graph load succeeds.
                Some(configured @ CliSource::Http(_)) => Some(self.sources.for_cli(configured)),
                _ => None,
            })
    }

    fn reload_project_catalog(&mut self) {
        if self.pending.project_catalog.is_some() {
            return;
        }
        let Some(base) = self.gateway("Для каталога проектов", |app, error| {
            app.scope.catalog_error = Some(error)
        }) else {
            return;
        };
        self.scope.catalog_error = None;
        let seq = self.next_seq();
        self.pending.project_catalog.set(seq);
        self.worker
            .send(WorkerCmd::LoadProjectCatalog { seq, base });
    }

    fn refresh_activity(&mut self) {
        if self.pending.activity.is_some() {
            return;
        }
        let Some(base) = self.gateway("Для журнала", |app, error| {
            app.activity.error = Some(error)
        }) else {
            return;
        };
        let seq = self.next_seq();
        self.pending.activity.set(seq);
        self.worker.send(WorkerCmd::LoadActivity { seq, base });
    }

    fn reload_project_home(&mut self) {
        let project = self.scope.wing.trim().to_string();
        self.home.overview = None;
        self.home.error = None;
        self.home.project = None;
        if project.is_empty() {
            self.pending.project_home.clear();
            return;
        }
        let Some(base) = self.gateway("Для пульта проекта", |app, error| {
            app.home.error = Some(error)
        }) else {
            return;
        };
        let seq = self.next_seq();
        self.pending.project_home.set(seq);
        self.worker
            .send(WorkerCmd::LoadProjectHome { seq, base, project });
    }

    fn ensure_project_home_loaded(&mut self) {
        let project = nonempty(&self.scope.wing);
        if self.pending.project_home.is_none() && self.home.project != project {
            self.reload_project_home();
        }
    }

    fn reload_library(&mut self) {
        let Some(base) = self.gateway("Для корпуса", |app, error| {
            app.library.error = Some(error);
            app.library.page = None;
        }) else {
            return;
        };
        self.library.error = None;
        self.library.request.wing = self.scope.wing.trim().to_string();
        self.library.request.limit = self.library.request.limit.clamp(1, 200);
        let request = self.library.request.clone();
        let seq = self.next_seq();
        self.pending.library.set(seq);
        self.worker
            .send(WorkerCmd::LoadLibrary { seq, base, request });
    }

    fn ensure_library_loaded(&mut self) {
        if self.pending.library.is_none() && self.library.page.is_none() {
            self.reload_library();
        }
    }

    fn reset_library_for_filters(&mut self) {
        self.library.request.cursor = None;
        self.library.cursor_history.clear();
        self.library.selected_id = None;
        self.library.document = None;
        self.library.document_error = None;
        self.pending.library_document.clear();
        self.reload_library();
    }

    fn select_library_document(&mut self, id: &str) {
        let item = self
            .library
            .page
            .as_ref()
            .and_then(|page| page.items.iter().find(|item| item.id == id))
            .cloned();
        let Some(item) = item else {
            return;
        };
        self.library.selected_id = Some(item.id.clone());
        self.library.document = None;
        self.library.document_error = None;
        let Some(base) = self.gateway("Для превью документа", |app, error| {
            app.library.document_error = Some(error)
        }) else {
            return;
        };
        let seq = self.next_seq();
        self.pending.library_document.set(seq);
        self.worker.send(WorkerCmd::LoadLibraryDocument {
            seq,
            base,
            document_id: item.id,
            uri: item.uri,
        });
    }

    fn run_search(&mut self) {
        if self.search.request.query.trim().is_empty() {
            self.search.error = Some("Введите поисковый запрос".into());
            return;
        }
        let Some(base) = self.gateway("Для поиска", |app, error| {
            app.search.error = Some(error)
        }) else {
            return;
        };
        self.search.request.wing = nonempty(&self.scope.wing);
        self.search.request.top_k = self.search.request.top_k.clamp(1, 100);
        self.search.error = None;
        let seq = self.next_seq();
        self.pending.search.set(seq);
        self.worker.send(WorkerCmd::Search {
            seq,
            base,
            request: self.search.request.clone(),
        });
    }

    fn reload_operations(&mut self) {
        let Some(base) = self.gateway("Для операций", |app, error| {
            app.ops.error = Some(error)
        }) else {
            return;
        };
        self.ops.error = None;
        let seq = self.next_seq();
        self.pending.operations.set(seq);
        self.worker.send(WorkerCmd::LoadOperations { seq, base });
    }

    fn reload_jobs(&mut self) {
        if self.pending.jobs.is_some() {
            return;
        }
        let Some(base) = self.gateway("Для фоновых задач", |app, error| {
            app.ops.error = Some(error)
        }) else {
            return;
        };
        let seq = self.next_seq();
        self.pending.jobs.set(seq);
        self.worker.send(WorkerCmd::LoadJobs { seq, base });
    }

    fn dispatch_operations_action(&mut self, action: OperationsAction) {
        if is_operations_mutation(&action) && self.pending.operation_action.is_some() {
            return;
        }
        match action {
            OperationsAction::None => {}
            OperationsAction::RefreshHealth => self.reload_operations(),
            OperationsAction::RefreshJobs => self.reload_jobs(),
            OperationsAction::StartSync(request) => {
                let Some(base) = self.gateway("Для фоновых задач", |app, error| {
                    app.ops.error = Some(error)
                }) else {
                    return;
                };
                let seq = self.next_seq();
                self.pending.operation_action.set(seq);
                self.worker
                    .send(WorkerCmd::StartSyncJob { seq, base, request });
            }
            OperationsAction::CancelJob(id) => {
                let Some(base) = self.gateway("Для фоновых задач", |app, error| {
                    app.ops.error = Some(error)
                }) else {
                    return;
                };
                let seq = self.next_seq();
                self.pending.operation_action.set(seq);
                self.worker.send(WorkerCmd::CancelJob { seq, base, id });
            }
            OperationsAction::Checkpoint => {
                let Some(base) = self.gateway("Для обслуживания", |app, error| {
                    app.ops.error = Some(error)
                }) else {
                    return;
                };
                let seq = self.next_seq();
                self.pending.operation_action.set(seq);
                self.worker.send(WorkerCmd::Checkpoint { seq, base });
            }
            OperationsAction::Backup(request) => {
                let Some(base) = self.gateway("Для обслуживания", |app, error| {
                    app.ops.error = Some(error)
                }) else {
                    return;
                };
                let seq = self.next_seq();
                self.pending.operation_action.set(seq);
                self.worker.send(WorkerCmd::Backup { seq, base, request });
            }
        }
    }

    fn reset_product_for_project(&mut self) {
        self.worker.cancel_project_scoped_reads();
        self.mode = mode_after_project_switch(self.mode);
        self.pending.graph.clear();
        self.pending.project_home.clear();
        self.pending.library.clear();
        self.pending.library_document.clear();
        self.pending.search.clear();
        self.pending.content.clear();
        self.pending.expand.clear();
        self.revision.clear();
        self.pending.revisions.clear();
        self.pending.revision_snapshot.clear();
        self.pending.revision_diff.clear();
        self.home.overview = None;
        self.home.project = None;
        self.home.error = None;
        self.library.request.wing = self.scope.wing.trim().to_string();
        self.library.page = None;
        self.library.error = None;
        self.library.request.cursor = None;
        self.library.cursor_history.clear();
        self.library.selected_id = None;
        self.library.open_after_load = None;
        self.library.document = None;
        self.library.document_error = None;
        self.search.request.wing = nonempty(&self.scope.wing);
        self.search.results = None;
        self.search.error = None;
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
        let selected = self.library.selected_id.as_deref()?;
        self.library
            .page
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
        self.graph.load_error = None;
        self.pending.expand.clear();
        self.graph.full_view = None;
        self.graph.full_view_rev += 1;
        self.graph.local_view = None;
        self.graph.local_truncated = false;
        self.graph.ui_graph = None;
        self.graph.positions.clear();
        self.graph.layout_ready = false;
        self.graph.need_fit = true;
        self.select_node(None, InFlightContent::Cancel);
        self.graph.expand_note = None;
        self.graph.expanded_dirty = false;
        self.graph.raw_truncated = false;
        self.graph.raw_node_count = 0;
        let seq = self.next_seq();
        self.pending.graph.set(seq);
        if refresh_project_catalog {
            self.reload_project_catalog();
        }
        self.worker.send(WorkerCmd::LoadGraph {
            seq,
            source: self.sources.for_cli(&src),
            seed: nonempty(&self.graph.seed_input),
            depth: self.graph.depth,
            project: nonempty(&self.scope.wing),
            include_tags: self.graph.show_tags,
        });
    }

    fn submit_graph_focus(&mut self) {
        self.graph.lens = GraphLens::Neighborhood;
        if self.topology_query_requires_reload() {
            self.graph.seed_id = None;
            self.graph.seed_error = None;
            self.dispatch_graph_load();
        } else {
            self.apply_seed_from_input();
        }
    }

    fn open_graph_focus(&mut self, query: String, fallback_label: Option<String>) {
        self.graph.seed_input = query;
        self.mode = ViewMode::Graph;
        self.submit_graph_focus();
        if self.activity_base().is_none() && self.graph.seed_error.is_some() {
            if let Some(fallback) = fallback_label {
                self.graph.seed_input = fallback;
                self.submit_graph_focus();
            }
        }
    }

    /// Switch to an HTTP source from the no-source start screen (no restart).
    fn connect_http(&mut self) {
        let url = self.connect_url.trim().to_string();
        if url.is_empty() {
            self.graph.load_error = Some("HTTP URL не задан".into());
            return;
        }
        self.connect_url.clone_from(&url);
        self.open.source = Some(CliSource::Http(url));
        self.graph.load_error = None;
        self.scope.catalog.clear();
        self.scope.catalog_error = None;
        self.wiki.loaded = false;
        self.home.overview = None;
        self.home.project = None;
        self.library.page = None;
        self.dispatch_graph_load();
    }

    fn apply_loaded(&mut self, loaded: LoadedGraph) {
        self.graph.load_error = None;
        self.graph.raw_truncated = loaded.truncated;
        self.graph.raw_node_count = loaded.raw_node_count;
        self.ops.health = loaded.health;
        if let Some(projects) = loaded.projects {
            self.scope.catalog = projects;
            self.scope.catalog_error = None;
        }
        self.source = Some(loaded.source);
        self.graph.full_view = Some(loaded.view);
        self.graph.full_view_rev += 1;
        if !self.graph.seed_input.is_empty() {
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
        if let Some(err) = &self.graph.load_error {
            return Some((EmptyKind::LoadError, Some(err.clone())));
        }
        if self.open.source.is_none() && self.graph.full_view.is_none() {
            return Some((EmptyKind::NoSource, None));
        }
        if let Some(full) = &self.graph.full_view {
            if full.nodes.is_empty() {
                return Some((EmptyKind::EmptyGraph, None));
            }
            if self.graph.seed_id.is_none() && self.graph.seed_error.is_some() {
                return Some((EmptyKind::SeedNotFound, self.graph.seed_error.clone()));
            }
            if self.graph.seed_id.is_none() && self.graph.lens == GraphLens::Neighborhood {
                return Some((EmptyKind::MissingSeed, None));
            }
            if let Some(g) = &self.graph.ui_graph {
                if g.nodes.is_empty() {
                    return Some((EmptyKind::FiltersEmpty, None));
                }
            }
        }
        None
    }

    /// Topology stats for empty banners when a source is loaded.
    fn empty_stats(&self) -> Option<EmptyGraphStats> {
        let full = self.graph.full_view.as_ref()?;
        let mut cache = self.graph.empty_stats_cache.borrow_mut();
        if let Some((rev, stats)) = cache.as_ref() {
            if *rev == self.graph.full_view_rev {
                return Some(stats.clone());
            }
        }
        let stats =
            EmptyGraphStats::from_view(full, self.graph.raw_node_count, self.graph.raw_truncated);
        *cache = Some((self.graph.full_view_rev, stats.clone()));
        Some(stats)
    }

    fn status_banner(&self) -> Option<String> {
        if let Some(e) = &self.graph.seed_error {
            return Some(e.clone());
        }
        if let Some(e) = &self.graph.expand_note {
            return Some(e.clone());
        }
        if self.graph.local_truncated {
            return Some(format!(
                "Локальное окружение достигло max_nodes ({}); часть достижимых узлов скрыта",
                self.graph.max_nodes
            ));
        }
        if let Some(note) = self.graph.ui_graph.as_ref().and_then(|g| g.note.clone()) {
            return Some(note);
        }
        if self.graph.raw_truncated {
            return Some(format!(
                "Показан локальный вид; исходный снимок содержал {} узлов (жёсткий лимит {})",
                self.graph.raw_node_count, UI_HARD_MAX_NODES
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
            && self.wiki.edit.is_some()
            && self.wiki_can_write()
            && self.pending.save.is_none()
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
        if esc && self.mode == ViewMode::Wiki && self.wiki.edit.is_some() {
            self.cancel_wiki_edit();
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if esc && self.mode == ViewMode::Graph && self.graph.selected.is_some() {
            self.select_node(None, InFlightContent::Cancel);
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
            self.wiki.edit.as_ref().is_some_and(|edit| edit.dirty),
            self.pending.save.is_some(),
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
