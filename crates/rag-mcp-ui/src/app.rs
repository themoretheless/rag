//! GraphApp: eframe::App - graph inspector + Obsidian/Notion-style wiki browser.
//!
//! All blocking IO (HTTP / DuckDB) runs on the worker thread (`crate::worker`);
//! this file only dispatches [`WorkerCmd`] and applies [`WorkerEvt`] in `update()`.
//! Each job carries a `seq`; late answers whose seq no longer matches the pending
//! slot are dropped (race protection per EGUI_GRAPH_VIEW §2.5 / §8.3).

use egui::Vec2;
use rag_mcp::GraphView;

use crate::adapter::{adapt, topology_generation, AdaptOptions, UiGraph};
use crate::layout::{place_missing_near_neighbors, radial_place, PosCache};
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::load::{
    local_neighbors_bounded, resolve_seed, sort_wiki_pages, ActivityEvent, CliSource, DocumentBody,
    GatewayHealth, GraphSourceKind, LoadedGraph, OpenArgs, WikiPageMeta, WikiPutRequest,
    UI_HARD_MAX_NODES,
};
use crate::operations::{JobSnapshot, MaintenanceResult, OperationsSnapshot};
use crate::product::{LibraryItem, LibraryPage, LibraryRequest, ProjectHome};
use crate::revisions::{RestoreRevisionResult, RevisionDiff, RevisionItem};
use crate::search::{SearchRequest, SearchResults};
use crate::ui::canvas::draw_canvas;
use crate::ui::detail::{draw_detail, DetailAction};
use crate::ui::empty::{
    draw_empty_banner, draw_no_source, EmptyGraphStats, EmptyKind, NoSourceAction,
};
use crate::ui::home::{draw_project_home, HomeAction};
use crate::ui::library::{
    draw_library_detail, draw_library_workspace, LibraryAction, LibraryDetailAction,
};
use crate::ui::operations::{
    draw_jobs, draw_maintenance, BackupForm, OperationsAction, OperationsTab, SyncJobForm,
};
use crate::ui::revisions::{draw_revisions_workspace, RevisionsAction, RevisionsView};
use crate::ui::search::{draw_search_workspace, SearchAction};
use crate::ui::status::draw_status;
use crate::ui::wiki::{
    content_summary_line, draw_wiki_edit_view, draw_wiki_info_panel, draw_wiki_read_view,
    draw_wiki_sidebar, slug_from_wiki_uri, wiki_filter_id, WikiEditBuffers, WikiReadContext,
};
use crate::worker::{LoadSource, WorkerCmd, WorkerEvt, WorkerHandle};

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
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
}

/// Which wiki article column is focused (sidebar / links land here when dual-pane is on).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum WikiPane {
    #[default]
    A,
    B,
}

fn mode_after_project_switch(mode: ViewMode) -> ViewMode {
    if mode == ViewMode::Revisions {
        ViewMode::Library
    } else {
        mode
    }
}

fn should_select_default_project(bootstrap_done: bool, project: &str, seed: &str) -> bool {
    !bootstrap_done && project.trim().is_empty() && seed.trim().is_empty()
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

fn prepare_library_request_from_search(
    request: &mut LibraryRequest,
    project: &str,
    title: &str,
    uri: &str,
) {
    request.clear_filters();
    request.wing = project.trim().to_string();
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

    full_view: Option<GraphView>,
    project_catalog: Vec<String>,
    /// True after the first successful source bootstrap. This distinguishes an
    /// initial default from the user's later explicit "All projects" choice.
    project_bootstrap_done: bool,
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
    wiki_backlinks_b: Vec<crate::load::BacklinkItem>,
}

impl GraphApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: OpenArgs) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
            style.spacing.interact_size.y = 30.0;
            style.text_styles.insert(
                egui::TextStyle::Heading,
                egui::FontId::new(24.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Body,
                egui::FontId::new(16.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::new(15.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Small,
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
            );
        });
        let depth = open.depth.clamp(1, 3);
        let max_nodes = open.max_nodes.clamp(1, UI_HARD_MAX_NODES as u32);
        let seed_input = open.seed.clone().unwrap_or_default();
        let mut app = Self {
            open,
            worker: crate::worker::spawn(cc.egui_ctx.clone()),
            seq: 0,
            pending_graph: None,
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
            connect_url: "http://127.0.0.1:7432".into(),
            full_view: None,
            project_catalog: Vec::new(),
            project_bootstrap_done: false,
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
            wiki_edit: None,
            wiki_save_note: None,
            wiki_dual_pane: false,
            wiki_focus: WikiPane::A,
            wiki_selected_id_b: None,
            wiki_article_b: None,
            wiki_error_b: None,
            wiki_backlinks_b: Vec::new(),
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

    fn activity_base(&self) -> Option<String> {
        match self.source.as_ref() {
            Some(GraphSourceKind::HttpService { base }) => Some(base.clone()),
            _ => None,
        }
    }

    fn refresh_activity(&mut self) {
        if self.pending_activity.is_some() {
            return;
        }
        let Some(base) = self.activity_base() else {
            self.activity_error = Some("Activity requires an HTTP gateway connection".into());
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
            self.project_home_error = Some("Select a project in the top bar".into());
            return;
        }
        let Some(base) = self.activity_base() else {
            self.project_home_error =
                Some("Project Home requires an HTTP gateway connection".into());
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
            self.library_error = Some("Unified Library requires an HTTP gateway connection".into());
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
                Some("Document preview requires an HTTP gateway connection".into());
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
            self.search_error = Some("Enter a search query".into());
            return;
        }
        let Some(base) = self.activity_base() else {
            self.search_error = Some("Search requires an HTTP gateway connection".into());
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
            self.operations_error = Some("Operations require an HTTP gateway connection".into());
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
            self.operations_error = Some("Jobs require an HTTP gateway connection".into());
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
                    self.operations_error = Some("Jobs require an HTTP gateway connection".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker
                    .send(WorkerCmd::StartSyncJob { seq, base, request });
            }
            OperationsAction::CancelJob(id) => {
                let Some(base) = self.activity_base() else {
                    self.operations_error = Some("Jobs require an HTTP gateway connection".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker.send(WorkerCmd::CancelJob { seq, base, id });
            }
            OperationsAction::Checkpoint => {
                let Some(base) = self.activity_base() else {
                    self.operations_error =
                        Some("Maintenance requires an HTTP gateway connection".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker.send(WorkerCmd::Checkpoint { seq, base });
            }
            OperationsAction::Backup(request) => {
                let Some(base) = self.activity_base() else {
                    self.operations_error =
                        Some("Maintenance requires an HTTP gateway connection".into());
                    return;
                };
                let seq = self.next_seq();
                self.pending_operation_action = Some(seq);
                self.worker.send(WorkerCmd::Backup { seq, base, request });
            }
        }
    }

    fn open_revisions(&mut self, item: &LibraryItem) {
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
            self.revision.error = Some("Open History from a Library document".into());
            return;
        };
        let Some(base) = self.activity_base() else {
            self.revision.error = Some("Revision history requires an HTTP gateway".into());
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
            self.revision.error = Some("Revision details require an HTTP gateway".into());
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
                "Raw documents are source-controlled. Restore the source file, then sync it to create a new indexed revision."
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
                Some("Restore needs the current document revision and HTTP gateway".into());
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
        self.pending_revision_restore = None;
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
            | ViewMode::Activity => {}
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
        if matches!(self.source, Some(GraphSourceKind::HttpService { .. })) {
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
        if !matches!(self.source, Some(GraphSourceKind::HttpService { .. }))
            && self.seed_error.is_some()
        {
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
            self.load_error = Some("http URL is empty".into());
            return;
        }
        self.open.source = Some(CliSource::Http(url));
        self.load_error = None;
        self.project_bootstrap_done = false;
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
        self.project_catalog = loaded.projects;
        self.source = Some(loaded.source);
        self.full_view = Some(loaded.view);
        let mut selected_default_project = false;
        if should_select_default_project(
            self.project_bootstrap_done,
            &self.filter_wing,
            &self.seed_input,
        ) {
            if let Some(project) = self.project_catalog.first() {
                self.filter_wing = project.clone();
                self.prev_filter_wing = project.clone();
                self.library_request.wing = project.clone();
                self.search_request.wing = Some(project.clone());
                selected_default_project = true;
            }
        }
        self.project_bootstrap_done = true;
        if selected_default_project
            && matches!(self.source, Some(GraphSourceKind::HttpService { .. }))
        {
            // The bootstrap request discovers the project catalog. Immediately
            // replace its bounded global graph with a server-scoped export.
            self.dispatch_graph_load();
            return;
        }
        if !self.seed_input.is_empty() {
            self.apply_seed_from_input();
        } else {
            self.rebuild_ui_graph();
        }
        match self.mode {
            ViewMode::Home => self.reload_project_home(),
            ViewMode::Library => self.reload_library(),
            ViewMode::Wiki => self.ensure_wiki_loaded(),
            ViewMode::Activity => self.refresh_activity(),
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
        match self.source.as_ref() {
            Some(s) => match LoadSource::from_graph_source(s) {
                Some(source) => {
                    let seq = self.next_seq();
                    self.pending_catalog = Some(seq);
                    self.worker.send(WorkerCmd::LoadWikiCatalog {
                        seq,
                        source,
                        project: nonempty(&self.filter_wing),
                    });
                }
                None => {
                    self.wiki_loaded = true;
                    self.wiki_error =
                        Some("snapshot mode has no wiki catalog; use --http or --db".into());
                }
            },
            None => {
                self.wiki_loaded = true;
                self.wiki_error = Some("no data source".into());
            }
        }
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
        // Same for pane A.
        if self.wiki_selected_id.as_deref() == Some(id) && self.wiki_article.is_some() {
            return;
        }
        if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error =
                Some("unsaved edits: Save or Cancel before opening another page".into());
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
        let Some(source) = self.source.as_ref().and_then(LoadSource::from_graph_source) else {
            self.wiki_error = Some("wiki articles require --http or --db".into());
            return;
        };
        let Some(meta) = self.wiki_pages.iter().find(|p| p.id == id).cloned() else {
            self.wiki_error = Some(format!("page id {id} not in catalog"));
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
        let Some(source) = self.source.as_ref().and_then(LoadSource::from_graph_source) else {
            self.wiki_error_b = Some("wiki articles require --http or --db".into());
            return;
        };
        let Some(meta) = self.wiki_pages.iter().find(|p| p.id == id).cloned() else {
            self.wiki_error_b = Some(format!("page id {id} not in catalog"));
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
        self.wiki_backlinks_b.clear();
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
        result: Result<(DocumentBody, Vec<crate::load::BacklinkItem>), String>,
    ) {
        match result {
            Ok((body, backlinks)) => {
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
                    self.wiki_backlinks_b = backlinks;
                    self.wiki_error_b = None;
                } else {
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
                    self.wiki_backlinks = backlinks;
                    self.wiki_error = None;
                    self.wiki_save_note = None;
                }
            }
            Err(e) => {
                let msg = match q {
                    Some(q) => format!(
                        "unresolved link [[{q}]] - no page with that title/slug (create via write_wiki_page): {e}"
                    ),
                    None => e,
                };
                if pane_b {
                    self.wiki_article_b = None;
                    self.wiki_backlinks_b.clear();
                    self.wiki_error_b = Some(msg);
                } else {
                    self.wiki_article = None;
                    self.wiki_backlinks.clear();
                    self.wiki_error = Some(msg);
                }
            }
        }
    }

    fn wiki_can_write(&self) -> bool {
        matches!(
            self.source.as_ref(),
            Some(GraphSourceKind::HttpService { .. } | GraphSourceKind::LiveStore { .. })
        )
    }

    fn start_wiki_edit(&mut self) {
        let Some(art) = self.wiki_article.as_ref() else {
            return;
        };
        if !self.wiki_can_write() {
            self.wiki_error = Some("editing requires --http or --db".into());
            return;
        }
        self.wiki_save_note = None;
        self.wiki_error = None;
        self.wiki_edit = Some(WikiEditBuffers::from_article(art));
    }

    fn cancel_wiki_edit(&mut self) {
        self.wiki_edit = None;
        self.wiki_error = None;
    }

    /// Discard edits and refetch the current revision (409 CAS conflict path).
    fn reload_wiki_page(&mut self) {
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
            self.wiki_error = Some("no page open to save".into());
            return;
        };
        let Some(source) = self.source.as_ref().and_then(LoadSource::from_graph_source) else {
            self.wiki_error = Some("wiki save requires --http or --db".into());
            return;
        };
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
                    .map(|r| format!(" saved r{r}"))
                    .unwrap_or_default();
                self.wiki_save_note = Some(format!("Saved “{}”{rev_note}", body.title));
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
        let Some(source) = self.source.as_ref().and_then(LoadSource::from_graph_source) else {
            return;
        };
        let seq = self.next_seq();
        self.pending_backlinks = Some(seq);
        self.worker.send(WorkerCmd::LoadBacklinks {
            seq,
            document_id: document_id.to_string(),
            source,
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
        if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("unsaved edits: Save or Cancel before going back".into());
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
        if !into_b && self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("unsaved edits: Save or Cancel before following a link".into());
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
        // Exact wiki uri fallback (no fuzzy label pick) - fetched on the worker.
        let Some(source) = self.source.as_ref().and_then(LoadSource::from_graph_source) else {
            let msg = "wiki links require --http or --db".to_string();
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

    fn open_selected_graph_node_in_wiki(&mut self) {
        let Some(sel) = self.selected.clone() else {
            return;
        };
        let (doc_id, uri, label) = {
            let Some(g) = self.ui_graph.as_ref() else {
                return;
            };
            let Some(node) = g.nodes.iter().find(|n| n.id == sel) else {
                return;
            };
            (
                node.document_id.clone(),
                node.uri.clone(),
                node.label.clone(),
            )
        };
        self.mode = ViewMode::Wiki;
        self.wiki_focus = WikiPane::A;
        self.ensure_wiki_loaded();
        if let Some(doc_id) = doc_id.as_deref() {
            if self.wiki_pages.iter().any(|p| p.id == doc_id) {
                self.open_wiki_page_id(doc_id);
                return;
            }
        }
        if let Some(uri) = uri.as_deref() {
            if uri.starts_with("wiki://") {
                if let Some(p) = self.wiki_pages.iter().find(|p| p.uri == uri) {
                    let id = p.id.clone();
                    self.open_wiki_page_id(&id);
                    return;
                }
            }
        }
        self.open_wiki_link(label.as_str());
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
            self.seed_error = Some("no graph loaded".into());
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
            self.content_error = Some("nothing selected".into());
            return;
        };
        let Some(g) = self.ui_graph.as_ref() else {
            self.content_error = Some("no graph view".into());
            return;
        };
        let Some(node) = g.nodes.iter().find(|n| n.id == sel) else {
            self.content_error = Some("selection not in view".into());
            return;
        };
        if node.document_id.is_none() && node.uri.is_none() {
            self.content_error = Some(format!(
                "node “{}” has no document ({} / unresolved stub?)",
                node.label, node.kind
            ));
            return;
        }
        let (doc_id, uri, label) = (
            node.document_id.clone(),
            node.uri.clone(),
            node.label.clone(),
        );

        let Some(source) = self.source.as_ref().and_then(LoadSource::from_graph_source) else {
            self.content_error =
                Some("snapshot mode has no document bodies; use --http or --db".into());
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
            self.local_view = None;
            self.local_truncated = false;
            self.ui_graph = Some(UiGraph::default());
            self.layout_ready = false;
            return;
        };

        let local = local_neighbors_bounded(full, seed, self.depth, self.max_nodes as usize);
        self.local_truncated = local.capped;
        self.apply_local_topology(local.view, true);
    }

    /// Adapt `local`, update layout. `reset_layout` re-runs RadialLocal; expand uses false.
    fn apply_local_topology(&mut self, local: GraphView, reset_layout: bool) {
        let seed = self.seed_id.clone();
        let opts = AdaptOptions {
            seed_id: seed.clone(),
            show_tags: self.show_tags,
            show_stubs: self.show_stubs,
            pkb_rels_only: true,
            wing: nonempty(&self.filter_wing),
            room: nonempty(&self.filter_room),
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
            self.expand_note = Some(format!("Expand blocked: already at max_nodes ({max_n})"));
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
            self.expand_note = Some("Expand requires a loaded graph".into());
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
            if self.seed_id.is_none() {
                if self.seed_error.is_some() {
                    return Some((EmptyKind::SeedNotFound, self.seed_error.clone()));
                }
                if self.raw_truncated {
                    return Some((
                        EmptyKind::OverCap,
                        Some(format!(
                            "server result reached the export limit ({} nodes); focus an item to load its neighborhood",
                            self.raw_node_count
                        )),
                    ));
                }
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
                "Local neighborhood reached max_nodes ({}); reachable nodes were omitted",
                self.max_nodes
            ));
        }
        if let Some(note) = self.ui_graph.as_ref().and_then(|g| g.note.clone()) {
            return Some(note);
        }
        if self.raw_truncated {
            return Some(format!(
                "Showing local view; raw snapshot had {} nodes (hard cap {})",
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
                                        "document {id} was not returned by the catalog filter"
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
                            self.operations_error = None;
                        }
                        Err(error) => self.operations_error = Some(error),
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
                                    self.wiki_error = Some(format!(
                                        "document {id} is not present in the project wiki catalog"
                                    ));
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
                    let Ok(bl) = result else {
                        continue;
                    };
                    if self.wiki_selected_id.as_deref() == Some(document_id.as_str()) {
                        self.wiki_backlinks = bl;
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
                            "No new neighbors for “{selected}” within max_nodes ({max_n})"
                        ));
                    } else if after >= max_n {
                        self.expand_note = Some(format!(
                            "Expand filled view to max_nodes ({max_n}); some neighbors may be omitted"
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
        let (esc, save, find, back) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::F),
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
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if esc {
            if self.mode == ViewMode::Wiki && self.wiki_edit.is_some() {
                self.cancel_wiki_edit();
            } else if self.mode == ViewMode::Graph && self.selected.is_some() {
                self.selected = None;
                self.content = None;
                self.content_error = None;
                self.pending_content = None;
            }
        }
        if find && self.mode == ViewMode::Wiki {
            ctx.memory_mut(|m| m.request_focus(wiki_filter_id()));
        }
        if back && self.mode == ViewMode::Wiki {
            self.wiki_go_back();
        }
    }
}

impl eframe::App for GraphApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.drain_worker_events();
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

        egui::Panel::top("toolbar").show(root_ui, |ui| {
            // Stable first tier: product navigation and project scope. Wrapping
            // keeps every workspace reachable on narrow native windows.
            ui.horizontal_wrapped(|ui| {
                ui.strong(egui::RichText::new("Knowledge Base").size(18.0));
                ui.separator();
                // Product workspaces first; focused tools follow.
                if ui
                    .selectable_label(self.mode == ViewMode::Home, "Home")
                    .on_hover_text("Project inventory and health")
                    .clicked()
                {
                    self.mode = ViewMode::Home;
                    self.ensure_project_home_loaded();
                }
                if ui
                    .selectable_label(self.mode == ViewMode::Library, "Library")
                    .on_hover_text("All indexed documents")
                    .clicked()
                {
                    self.mode = ViewMode::Library;
                    self.ensure_library_loaded();
                }
                if ui
                    .selectable_label(self.mode == ViewMode::Search, "Search")
                    .on_hover_text("Hybrid, lexical and semantic retrieval")
                    .clicked()
                {
                    self.mode = ViewMode::Search;
                    self.search_request.wing = nonempty(&self.filter_wing);
                }
                if ui
                    .add_enabled(
                        self.revision.document_id.is_some(),
                        egui::Button::new("History").selected(self.mode == ViewMode::Revisions),
                    )
                    .on_hover_text("Revision timeline for the document opened from Library")
                    .clicked()
                {
                    self.mode = ViewMode::Revisions;
                    if self.revision.items.is_empty() {
                        self.reload_revisions();
                    }
                }
                if ui
                    .selectable_label(self.mode == ViewMode::Wiki, "Wiki")
                    .on_hover_text("Linked articles and notes")
                    .clicked()
                {
                    self.mode = ViewMode::Wiki;
                    self.ensure_wiki_loaded();
                }
                if ui
                    .selectable_label(self.mode == ViewMode::Graph, "Connections")
                    .on_hover_text("Local object graph")
                    .clicked()
                {
                    self.mode = ViewMode::Graph;
                }
                if ui
                    .selectable_label(self.mode == ViewMode::Activity, "Operations")
                    .on_hover_text("Live gateway activity and service health")
                    .clicked()
                {
                    self.mode = ViewMode::Activity;
                    match self.operations_tab {
                        OperationsTab::Activity => self.refresh_activity(),
                        OperationsTab::Jobs => self.reload_jobs(),
                        OperationsTab::Maintenance => self.reload_operations(),
                    }
                }
                ui.separator();

                ui.weak("Project");
                ui.add_enabled_ui(
                    !self.wiki_edit.as_ref().is_some_and(|edit| edit.dirty),
                    |ui| {
                        egui::ComboBox::from_id_salt("global_project")
                            .selected_text(if self.filter_wing.is_empty() {
                                "All projects"
                            } else {
                                &self.filter_wing
                            })
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                crate::ui::closing_selectable_value(
                                    ui,
                                    &mut self.filter_wing,
                                    String::new(),
                                    "All projects",
                                );
                                for project in &project_options {
                                    crate::ui::closing_selectable_value(
                                        ui,
                                        &mut self.filter_wing,
                                        project.clone(),
                                        project,
                                    );
                                }
                            });
                    },
                );
            });
            ui.add_space(2.0);

            // Contextual controls get their own responsive tier instead of
            // competing with the workspace tabs for one clipped row.
            ui.horizontal_wrapped(|ui| {
                match self.mode {
                    ViewMode::Home => {
                        if self.pending_project_home.is_some() {
                            ui.spinner();
                            ui.weak("inventory…");
                        } else if let Some(home) = &self.project_home {
                            ui.weak(format!("{} documents", home.documents));
                        }
                    }
                    ViewMode::Library => {
                        if self.pending_library.is_some() {
                            ui.spinner();
                            ui.weak("catalog…");
                        } else if let Some(page) = &self.library_page {
                            ui.weak(format!("{} documents", page.total));
                        }
                    }
                    ViewMode::Search => {
                        if self.pending_search.is_some() {
                            ui.spinner();
                            ui.weak("searching…");
                        } else if let Some(results) = &self.search_results {
                            ui.weak(format!("{} results", results.items.len()));
                        }
                    }
                    ViewMode::Revisions => {
                        if self.pending_revisions.is_some()
                            || self.pending_revision_snapshot.is_some()
                            || self.pending_revision_diff.is_some()
                            || self.pending_revision_restore.is_some()
                        {
                            ui.spinner();
                            ui.weak("history…");
                        } else if let Some(head) = self.revision.head {
                            ui.weak(format!("head r{head}"));
                        }
                    }
                    ViewMode::Activity => {
                        if ui
                            .selectable_label(
                                self.operations_tab == OperationsTab::Activity,
                                "Activity",
                            )
                            .clicked()
                        {
                            self.operations_tab = OperationsTab::Activity;
                            self.refresh_activity();
                        }
                        if ui
                            .selectable_label(self.operations_tab == OperationsTab::Jobs, "Jobs")
                            .clicked()
                        {
                            self.operations_tab = OperationsTab::Jobs;
                            self.reload_jobs();
                        }
                        if ui
                            .selectable_label(
                                self.operations_tab == OperationsTab::Maintenance,
                                "Health & backup",
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
                            if ui.button("Refresh").clicked() {
                                self.refresh_activity();
                            }
                            ui.checkbox(&mut self.activity_auto_refresh, "Live");
                        } else if self.operations_tab == OperationsTab::Jobs {
                            if self.pending_jobs.is_some()
                                || self.pending_operation_action.is_some()
                            {
                                ui.spinner();
                            }
                            ui.weak(format!("{} retained", self.operations_jobs.len()));
                        } else {
                            if self.pending_operations.is_some()
                                || self.pending_operation_action.is_some()
                            {
                                ui.spinner();
                            }
                            if let Some(snapshot) = &self.operations_snapshot {
                                ui.colored_label(
                                    if snapshot.doctor.ok {
                                        egui::Color32::from_rgb(90, 190, 125)
                                    } else {
                                        egui::Color32::from_rgb(220, 105, 90)
                                    },
                                    if snapshot.doctor.ok {
                                        "healthy"
                                    } else {
                                        "attention"
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
                                "Filters ●"
                            } else {
                                "Filters"
                            },
                            |ui| {
                                ui.set_min_width(260.0);
                                ui.label("Event type");
                                egui::ComboBox::from_id_salt("activity_kind_filter")
                                    .selected_text(match self.activity_kind_filter.as_str() {
                                        "http" => "HTTP",
                                        "mcp_tool" => "MCP tool",
                                        _ => "All types",
                                    })
                                    .show_ui(ui, |ui| {
                                        for (value, label) in [
                                            ("all", "All types"),
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
                                ui.label("Result");
                                egui::ComboBox::from_id_salt("activity_status_filter")
                                    .selected_text(match self.activity_status_filter.as_str() {
                                        "success" => "Success",
                                        "error" => "Errors",
                                        _ => "All results",
                                    })
                                    .show_ui(ui, |ui| {
                                        for (value, label) in [
                                            ("all", "All results"),
                                            ("success", "Success"),
                                            ("error", "Errors"),
                                        ] {
                                            crate::ui::closing_selectable_value(
                                                ui,
                                                &mut self.activity_status_filter,
                                                value.to_string(),
                                                label,
                                            );
                                        }
                                    });
                                ui.label("Client");
                                ui.text_edit_singleline(&mut self.activity_client_filter);
                                ui.label("Action or route");
                                ui.text_edit_singleline(&mut self.activity_action_filter);
                                ui.label("Any field");
                                ui.text_edit_singleline(&mut self.activity_filter);
                                if ui
                                    .add_enabled(filters_active, egui::Button::new("Clear filters"))
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
                    ViewMode::Wiki => {
                        if ui
                            .selectable_label(self.wiki_sidebar_visible, "☰ Pages")
                            .on_hover_text("Show or hide the page catalog")
                            .clicked()
                        {
                            self.wiki_sidebar_visible = !self.wiki_sidebar_visible;
                        }
                        if ui
                            .add_enabled(!self.wiki_history.is_empty(), egui::Button::new("← Back"))
                            .on_hover_text("Back in page history (Alt+←)")
                            .clicked()
                        {
                            self.wiki_go_back();
                        }
                        if ui.button("Reload wiki").clicked() {
                            if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
                                self.wiki_error =
                                    Some("unsaved edits: Save or Cancel before reload".into());
                            } else {
                                self.wiki_edit = None;
                                self.wiki_loaded = false;
                                self.reload_wiki_catalog();
                            }
                        }
                        if self.pending_catalog.is_some() {
                            ui.spinner();
                            ui.weak("catalog…");
                        }
                        if self.pending_page_a.is_some() || self.pending_page_b.is_some() {
                            ui.spinner();
                            ui.weak("page…");
                        }
                        if ui
                            .selectable_label(self.wiki_dual_pane, "Dual pane")
                            .on_hover_text(
                                "Two articles: catalog left, pane A center, pane B right",
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
                                .selectable_label(self.wiki_info_visible, "Info")
                                .on_hover_text("Show page metadata and backlinks")
                                .clicked()
                        {
                            self.wiki_info_visible = !self.wiki_info_visible;
                        }
                        if self.wiki_dual_pane {
                            if ui
                                .selectable_label(self.wiki_focus == WikiPane::A, "A")
                                .on_hover_text("Focus pane A (center); Edit/Save apply here")
                                .clicked()
                            {
                                self.wiki_focus = WikiPane::A;
                            }
                            if ui
                                .selectable_label(self.wiki_focus == WikiPane::B, "B")
                                .on_hover_text("Focus pane B (right); sidebar opens here")
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
                                    egui::Button::new("Edit"),
                                )
                                .on_hover_text("Edit pane A (Save via HTTP PUT or --db)")
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
                                    egui::Button::new("Save"),
                                )
                                .on_hover_text("Ctrl+S")
                                .clicked()
                            {
                                self.save_wiki_edit();
                            }
                            if saving {
                                ui.spinner();
                                ui.weak("saving…");
                            }
                            if ui.button("Cancel edit").on_hover_text("Esc").clicked() {
                                self.cancel_wiki_edit();
                            }
                        }
                        if ui
                            .add_enabled(
                                self.wiki_selected_id.is_some() && !editing,
                                egui::Button::new("Show in graph"),
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
                        ui.weak("Focus");
                        let seed_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.seed_input)
                                .desired_width(180.0)
                                .hint_text("Search an item…"),
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
                            // Enter: exact input, or first suggestion when one matches.
                            if let Some(id) = first_pick {
                                self.seed_input = id;
                            }
                            self.submit_graph_focus();
                        } else if let Some(id) = picked {
                            seed_resp.surrender_focus();
                            self.seed_input = id;
                            self.submit_graph_focus();
                        }
                        if ui.button("Show").clicked() {
                            self.submit_graph_focus();
                        }
                        ui.menu_button("View options", |ui| {
                            ui.set_min_width(220.0);
                            ui.horizontal(|ui| {
                                ui.label("Connection depth");
                                let mut d = self.depth as i32;
                                if ui.add(egui::DragValue::new(&mut d).range(1..=3)).changed() {
                                    self.depth = d as u32;
                                    if matches!(
                                        self.source,
                                        Some(GraphSourceKind::HttpService { .. })
                                    ) {
                                        self.dispatch_graph_load();
                                    } else {
                                        self.rebuild_ui_graph();
                                    }
                                }
                            });
                            ui.checkbox(&mut self.show_tags, "Show tags");
                            ui.checkbox(&mut self.show_stubs, "Show unresolved items");
                            ui.separator();
                            ui.label("Room");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.filter_room)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("All rooms"),
                            );
                            if ui
                                .add_enabled(
                                    !self.filter_room.is_empty(),
                                    egui::Button::new("Clear room filter"),
                                )
                                .clicked()
                            {
                                self.filter_room.clear();
                            }
                        });
                        if ui
                            .button("Reset view")
                            .on_hover_text(
                                "Rebuild the local view from the seed (drops Expand merges)",
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
                                egui::Button::new("Expand"),
                            )
                            .clicked()
                        {
                            self.expand_selected();
                        }
                        if expanding {
                            ui.spinner();
                        }
                        if ui
                            .add_enabled(self.selected.is_some(), egui::Button::new("Open page"))
                            .on_hover_text("Open selected node as article")
                            .clicked()
                        {
                            self.open_selected_graph_node_in_wiki();
                        }
                        if ui
                            .add_enabled(
                                self.selected.is_some() && self.pending_content.is_none(),
                                egui::Button::new("Preview"),
                            )
                            .clicked()
                        {
                            self.load_content_for_selected();
                        }
                        if self.pending_content.is_some() {
                            ui.spinner();
                        }
                    }
                }
            });
        });

        let project_changed = self.filter_wing != self.prev_filter_wing;
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
            let project_needs_reload =
                project_changed && matches!(self.source, Some(GraphSourceKind::HttpService { .. }));
            let tags_need_reload = tags_changed
                && matches!(
                    self.source,
                    Some(GraphSourceKind::HttpService { .. } | GraphSourceKind::LiveStore { .. })
                );
            if project_needs_reload || tags_need_reload {
                self.dispatch_graph_load();
            } else {
                self.rebuild_ui_graph();
            }
        }

        egui::Panel::bottom("status").show(root_ui, |ui| {
            match self.mode {
                ViewMode::Home => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Project Home");
                        ui.separator();
                        if let Some(project) = nonempty(&self.filter_wing) {
                            ui.label(project);
                        } else {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 150, 65),
                                "select a project",
                            );
                        }
                        if self.pending_project_home.is_some() {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Updating…");
                        }
                    });
                }
                ViewMode::Library => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Unified Library");
                        if let Some(page) = &self.library_page {
                            ui.separator();
                            ui.label(format!("{} total", page.total));
                            ui.label(format!("{} on page", page.items.len()));
                        }
                        if self.pending_library.is_some() || self.pending_library_document.is_some()
                        {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Updating…");
                        }
                    });
                }
                ViewMode::Search => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Search");
                        ui.separator();
                        ui.label(match self.search_request.mode.as_str() {
                            "lex" => "lexical",
                            "vec" => "semantic",
                            _ => "hybrid",
                        });
                        if let Some(project) = self.search_request.wing.as_deref() {
                            ui.separator();
                            ui.label(project);
                        }
                        if self.pending_search.is_some() {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Searching…");
                        }
                    });
                }
                ViewMode::Revisions => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Revision history");
                        if let Some(head) = self.revision.head {
                            ui.separator();
                            ui.label(format!("head r{head}"));
                        }
                        if let Some(selected) = self.revision.selected {
                            ui.separator();
                            ui.label(format!("comparing r{selected}"));
                        }
                        if self.pending_revisions.is_some()
                            || self.pending_revision_snapshot.is_some()
                            || self.pending_revision_diff.is_some()
                            || self.pending_revision_restore.is_some()
                        {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Updating…");
                        }
                    });
                }
                ViewMode::Activity => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Operations");
                        ui.separator();
                        match self.operations_tab {
                            OperationsTab::Activity => {
                                ui.label(format!("{} retained events", self.activity.len()));
                                ui.weak("bodies and secret headers are not recorded");
                            }
                            OperationsTab::Jobs => {
                                ui.label(format!("{} retained jobs", self.operations_jobs.len()));
                                ui.weak("single writer lane");
                            }
                            OperationsTab::Maintenance => {
                                let healthy = self
                                    .operations_snapshot
                                    .as_ref()
                                    .is_some_and(|snapshot| snapshot.doctor.ok);
                                ui.colored_label(
                                    if healthy {
                                        egui::Color32::from_rgb(90, 190, 125)
                                    } else {
                                        egui::Color32::from_rgb(220, 150, 65)
                                    },
                                    if healthy {
                                        "healthy"
                                    } else {
                                        "check diagnostics"
                                    },
                                );
                            }
                        }
                    });
                }
                ViewMode::Wiki => {
                    ui.horizontal_wrapped(|ui| {
                        let connected = self.source.is_some();
                        ui.colored_label(
                            if connected {
                                egui::Color32::from_rgb(90, 190, 125)
                            } else {
                                egui::Color32::from_rgb(220, 105, 90)
                            },
                            if connected {
                                "● Connected"
                            } else {
                                "● Offline"
                            },
                        )
                        .on_hover_text(
                            self.source
                                .as_ref()
                                .map(|source| source.label())
                                .unwrap_or("No data source"),
                        );
                        ui.separator();
                        ui.label(format!("{} pages", self.wiki_pages.len()));
                        if self.any_pending() {
                            ui.separator();
                            ui.spinner();
                            ui.weak("Updating…");
                        }
                        if self.wiki_dual_pane {
                            ui.separator();
                            ui.label("Split view");
                        }
                        if self.wiki_edit.is_some() {
                            ui.separator();
                            ui.strong("Editing");
                            if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
                                ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "unsaved");
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
            }
            if let Some(health) = &self.ops_health {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("ops");
                    let healthy = health.fts_ready
                        && health.relational_integrity_ok
                        && !health.wal_too_large
                        && health.documents_without_chunks == 0;
                    ui.colored_label(
                        if healthy {
                            egui::Color32::from_rgb(100, 180, 120)
                        } else {
                            egui::Color32::from_rgb(220, 120, 80)
                        },
                        if healthy { "healthy" } else { "attention" },
                    );
                    ui.label(format!(
                        "backend={} schema={} docs={} chunks={}",
                        health.backend, health.schema_version, health.documents, health.chunks
                    ));
                    ui.label(format!(
                        "wal={}/{} MiB",
                        health.wal_bytes / 1_048_576,
                        health.wal_warn_bytes / 1_048_576
                    ));
                    if health.documents_without_chunks > 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 120, 80),
                            format!("missing_chunks={}", health.documents_without_chunks),
                        );
                    }
                });
            }
        });

        match self.mode {
            ViewMode::Home => {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    if self.full_view.is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Connecting to the knowledge base…");
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
                        self.project_home_error.as_deref(),
                        self.pending_project_home.is_some(),
                    );
                    match action {
                        HomeAction::None => {}
                        HomeAction::Refresh => self.reload_project_home(),
                        HomeAction::OpenLibrary => {
                            self.mode = ViewMode::Library;
                            self.ensure_library_loaded();
                        }
                        HomeAction::OpenGraph => {
                            self.mode = ViewMode::Graph;
                            if matches!(self.source, Some(GraphSourceKind::HttpService { .. })) {
                                self.seed_input.clear();
                                self.seed_id = None;
                                self.dispatch_graph_load();
                            }
                        }
                    }
                });
            }
            ViewMode::Library => {
                if let Some(item) = self.selected_library_item() {
                    let mut detail_action = LibraryDetailAction::None;
                    egui::Panel::right("unified_library_detail")
                        .default_size(390.0)
                        .size_range(300.0..=720.0)
                        .resizable(true)
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
                    if self.full_view.is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Connecting to the knowledge base…");
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
                    if self.full_view.is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Connecting to the knowledge base…");
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
                        } => {
                            self.mode = ViewMode::Library;
                            prepare_library_request_from_search(
                                &mut self.library_request,
                                &self.filter_wing,
                                &title,
                                &uri,
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
                    ui.heading("RAG activity");
                    ui.label("Newest events first. Clients use stable anonymous identifiers; MCP events show the operation name without arguments or result content.");
                    if let Some(error) = &self.activity_error {
                        ui.colored_label(egui::Color32::from_rgb(220, 105, 90), error);
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
                                ui.strong("Local time");
                                ui.strong("Kind");
                                ui.strong("Client");
                                ui.strong("Action");
                                ui.strong("Status");
                                ui.strong("Duration");
                                ui.strong("Request ID");
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
                                    ui.label(&event.action).on_hover_text(format!("event #{}", event.seq));
                                    let status = event.status.map(|value| value.to_string()).unwrap_or_else(|| "—".into());
                                    ui.colored_label(
                                        if event.status.is_some_and(|value| value >= 400) { egui::Color32::from_rgb(220, 105, 90) } else { egui::Color32::from_rgb(100, 180, 120) },
                                        status,
                                    );
                                    ui.monospace(event.elapsed_ms.map(|ms| format!("{ms:.1} ms")).unwrap_or_else(|| "—".into()));
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
                            self.pending_operations.is_some()
                                || self.pending_operation_action.is_some(),
                        );
                    });
                    self.dispatch_operations_action(action);
                }
            },
            ViewMode::Wiki => {
                // Left: catalog (polished dual-pane nav column).
                if self.wiki_sidebar_visible {
                    egui::Panel::left("wiki_nav")
                        .default_size(250.0)
                        .size_range(190.0..=420.0)
                        .resizable(true)
                        .show_separator_line(false)
                        .frame(egui::Frame::side_top_panel(root_ui.style()).inner_margin(12.0))
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
                        .show(root_ui, |ui| {
                            ui.horizontal(|ui| {
                                let focused = self.wiki_focus == WikiPane::B;
                                if ui
                                    .selectable_label(focused, "Pane B")
                                    .on_hover_text("Secondary article (read-only)")
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
                                                .small_button("Clear")
                                                .on_hover_text("Clear secondary page")
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
                        .frame(egui::Frame::side_top_panel(root_ui.style()).inner_margin(14.0))
                        .show(root_ui, |ui| {
                            let action = draw_wiki_info_panel(
                                ui,
                                self.wiki_article.as_ref(),
                                &self.wiki_backlinks,
                            );
                            if let Some(id) = action.open_id {
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
                    if self.full_view.is_none() {
                        if self.pending_graph.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(80.0);
                                ui.spinner();
                                ui.label("Loading…");
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
                                .selectable_label(focused, "Pane A")
                                .on_hover_text("Primary article (edit applies here)")
                                .clicked()
                            {
                                self.wiki_focus = WikiPane::A;
                            }
                        });
                        ui.separator();
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
                    egui::Panel::right("detail")
                        .default_size(detail_w)
                        .show(root_ui, |ui| {
                            if let (Some(sel), Some(g)) =
                                (self.selected.as_deref(), self.ui_graph.as_ref())
                            {
                                let action = draw_detail(
                                    ui,
                                    g,
                                    sel,
                                    self.content.as_ref(),
                                    self.content_error.as_deref(),
                                    self.pending_content.is_some(),
                                );
                                match action {
                                    DetailAction::ReadContent => self.load_content_for_selected(),
                                    DetailAction::CloseContent => {
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
                            ui.label("Loading graph…");
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
            egui::Window::new("Reset to seed")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label("Drop nodes merged by Expand neighbors and rebuild from the seed?");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Reset").clicked() {
                            do_reset = true;
                            stay = false;
                        }
                        if ui.button("Cancel").clicked() {
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

        // Keep spinner animations / pending states alive while the worker runs.
        if self.any_pending() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn all_projects_remains_explicit_after_bootstrap() {
        assert!(should_select_default_project(false, "", ""));
        assert!(!should_select_default_project(true, "", ""));
        assert!(!should_select_default_project(false, "alpha", ""));
        assert!(!should_select_default_project(false, "", "doc://seed"));
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
        );
        assert_eq!(request.q, "file:///alpha/result.md");
        assert_eq!(request.wing, "alpha");
        assert!(request.room.is_empty());
        assert!(request.layer.is_empty());
        assert!(request.kind.is_empty());
        assert!(request.status.is_empty());
        assert!(!request.include_archived);
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
