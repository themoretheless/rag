//! Per-workspace state owned by [`GraphApp`](super::GraphApp).
//!
//! Each struct holds what one workspace (or one cross-cutting concern) needs
//! between frames. `GraphApp::new` composes them from `Default`, apart from the
//! graph view which starts from the CLI arguments.

use super::*;

/// Selected project / room and the authoritative project list.
#[derive(Debug, Default)]
pub(super) struct ProjectScope {
    pub(super) wing: String,
    pub(super) room: String,
    pub(super) prev_wing: String,
    pub(super) prev_room: String,
    pub(super) catalog: Vec<String>,
    pub(super) catalog_error: Option<String>,
}

/// Project Home dashboard.
#[derive(Debug, Default)]
pub(super) struct HomeState {
    pub(super) overview: Option<ProjectHome>,
    /// Project the overview (or its error) belongs to.
    pub(super) project: Option<String>,
    pub(super) error: Option<String>,
}

/// Unified Library: filtered page, cursor trail and the opened document.
#[derive(Debug)]
pub(super) struct LibraryState {
    pub(super) request: LibraryRequest,
    pub(super) page: Option<LibraryPage>,
    pub(super) error: Option<String>,
    pub(super) cursor_history: Vec<Option<String>>,
    pub(super) selected_id: Option<String>,
    pub(super) open_after_load: Option<String>,
    pub(super) document: Option<DocumentBody>,
    pub(super) document_error: Option<String>,
}

impl Default for LibraryState {
    fn default() -> Self {
        Self {
            request: LibraryRequest {
                limit: 50,
                ..LibraryRequest::default()
            },
            page: None,
            error: None,
            cursor_history: Vec::new(),
            selected_id: None,
            open_after_load: None,
            document: None,
            document_error: None,
        }
    }
}

/// Search workspace.
#[derive(Debug, Default)]
pub(super) struct SearchState {
    pub(super) request: SearchRequest,
    pub(super) results: Option<SearchResults>,
    pub(super) error: Option<String>,
}

/// Operations: gateway health, jobs and maintenance forms.
#[derive(Debug, Default)]
pub(super) struct OpsState {
    pub(super) health: Option<GatewayHealth>,
    pub(super) tab: OperationsTab,
    pub(super) snapshot: Option<OperationsSnapshot>,
    /// The last health snapshot is retained for diagnosis, but must never be
    /// presented as current after a failed status/doctor refresh.
    pub(super) snapshot_stale: bool,
    pub(super) jobs: Vec<JobSnapshot>,
    pub(super) error: Option<String>,
    pub(super) last_result: Option<MaintenanceResult>,
    pub(super) jobs_last_refresh: Option<Instant>,
    pub(super) sync_job_form: SyncJobForm,
    pub(super) backup_form: BackupForm,
}

/// Lowercased text the activity filters match against, one per event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActivitySearchKey {
    pub(super) client: String,
    pub(super) action: String,
    /// Timestamp, kind, client, action and status in one string.
    pub(super) any: String,
}

impl ActivitySearchKey {
    pub(super) fn of(event: &ActivityEvent) -> Self {
        let client = event.client.as_deref().unwrap_or("");
        Self {
            client: client.to_lowercase(),
            action: event.action.to_lowercase(),
            any: format!(
                "{} {} {} {} {}",
                event.at,
                event.kind,
                client,
                event.action,
                event.status.map(|v| v.to_string()).unwrap_or_default()
            )
            .to_lowercase(),
        }
    }
}

/// Gateway activity feed and its filters.
#[derive(Debug)]
pub(super) struct ActivityState {
    /// Replace only through [`ActivityState::set_events`].
    pub(super) events: Vec<ActivityEvent>,
    /// `search_keys[i]` belongs to `events[i]`; built when the feed is loaded
    /// instead of on every frame.
    pub(super) search_keys: Vec<ActivitySearchKey>,
    pub(super) error: Option<String>,
    pub(super) last_refresh: Option<Instant>,
    pub(super) auto_refresh: bool,
    pub(super) filter: String,
    pub(super) kind_filter: String,
    pub(super) client_filter: String,
    pub(super) action_filter: String,
    pub(super) status_filter: String,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            search_keys: Vec::new(),
            error: None,
            last_refresh: None,
            auto_refresh: true,
            filter: String::new(),
            kind_filter: "all".into(),
            client_filter: String::new(),
            action_filter: String::new(),
            status_filter: "all".into(),
        }
    }
}

impl ActivityState {
    /// Replace the feed and its search keys together.
    pub(super) fn set_events(&mut self, events: Vec<ActivityEvent>) {
        self.search_keys = events.iter().map(ActivitySearchKey::of).collect();
        self.events = events;
    }
}

/// One wiki article column.
#[derive(Debug, Default)]
pub(super) struct WikiPane {
    pub(super) selected_id: Option<String>,
    pub(super) article: Option<DocumentBody>,
    pub(super) error: Option<String>,
}

/// Wiki browser: catalog, the two article panes, history and the editor.
pub(super) struct WikiState {
    pub(super) pages: Vec<WikiPageMeta>,
    /// Bumped on every `pages` change; keys the link-resolution cache.
    pub(super) pages_rev: u64,
    /// ASCII-lowercased titles and slugs for `pages_rev`, built once per
    /// catalog change instead of on every frame.
    pub(super) known_keys_cache: Option<(u64, std::rc::Rc<WikiKnownKeys>)>,
    /// Sidebar index for `pages_rev` (search text, category groups).
    pub(super) sidebar_index_cache: Option<(u64, std::rc::Rc<WikiSidebarIndex>)>,
    pub(super) filter: String,
    pub(super) show_summaries: bool,
    pub(super) sidebar_visible: bool,
    pub(super) info_visible: bool,
    /// Unified Library page to open after a project-scoped wiki catalog refresh.
    pub(super) open_after_catalog: Option<String>,
    pub(super) loaded: bool,
    /// History stack of wiki page ids for Back (Obsidian-like).
    pub(super) history: Vec<String>,
    pub(super) backlinks: Vec<crate::dto::BacklinkItem>,
    pub(super) backlinks_error: Option<String>,
    /// In-app editor buffers (None = read mode). Applies to pane A only.
    pub(super) edit: Option<WikiEditBuffers>,
    /// Last successful save note (status strip).
    pub(super) save_note: Option<String>,
    /// Two-article layout: catalog left, pane A center, pane B right SidePanel.
    pub(super) dual_pane: bool,
    /// Focused column for sidebar clicks and wikilink navigation.
    pub(super) focus: WikiFocus,
    /// Primary article pane.
    pub(super) a: WikiPane,
    /// Secondary article pane (dual-pane layout only).
    pub(super) b: WikiPane,
}

impl Default for WikiState {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            pages_rev: 0,
            known_keys_cache: None,
            sidebar_index_cache: None,
            filter: String::new(),
            show_summaries: false,
            sidebar_visible: true,
            info_visible: true,
            open_after_catalog: None,
            loaded: false,
            history: Vec::new(),
            backlinks: Vec::new(),
            backlinks_error: None,
            edit: None,
            save_note: None,
            dual_pane: false,
            focus: WikiFocus::A,
            a: WikiPane::default(),
            b: WikiPane::default(),
        }
    }
}

/// Connections workspace: loaded topology, the local view drawn from it,
/// camera, selection and the selected node's document.
pub(super) struct GraphViewState {
    /// Loaded topology. Shared, not copied, with Expand jobs on the worker.
    pub(super) full_view: Option<Arc<GraphView>>,
    /// Bumped whenever `full_view` is replaced; keys the seed-picker cache.
    pub(super) full_view_rev: u64,
    /// Seed picker suggestions for (`full_view_rev`, lowercased query): the
    /// scan over the loaded graph (up to `UI_GRAPH_EXPORT_MAX_NODES`) runs
    /// once per query change, not every frame while the field has focus.
    pub(super) seed_suggestions_cache: Option<SeedSuggestions>,
    /// Lowercased label / id / document id per `full_view` node for
    /// `full_view_rev`, built on the first query after a load.
    pub(super) seed_search_keys: Option<(u64, Vec<SeedSearchKey>)>,
    /// Empty-banner topology stats for `full_view_rev`: counting kinds and
    /// relation types walks every loaded node and edge, so it runs once per
    /// load instead of every frame the banner shows.
    pub(super) empty_stats_cache: std::cell::RefCell<Option<(u64, EmptyGraphStats)>>,
    /// Wiki target of the selected graph node, memoized per (document id,
    /// uri, wiki `pages_rev`): resolving it scans the wiki catalog, and the
    /// toolbar asks every frame while a node is selected.
    pub(super) wiki_target_cache: std::cell::RefCell<Option<GraphWikiTargetMemo>>,
    pub(super) load_error: Option<String>,
    pub(super) raw_truncated: bool,
    pub(super) raw_node_count: usize,

    pub(super) seed_input: String,
    pub(super) seed_id: Option<String>,
    pub(super) seed_error: Option<String>,
    pub(super) depth: u32,
    pub(super) max_nodes: u32,
    pub(super) lens: GraphLens,

    pub(super) show_tags: bool,
    pub(super) show_stubs: bool,
    pub(super) prev_show_tags: bool,
    pub(super) prev_show_stubs: bool,

    /// Local topology (seed BFS and/or Expand merges) before adapter filters.
    pub(super) local_view: Option<Arc<GraphView>>,
    /// Exact local BFS budget truncation, independent from server/adapter caps.
    pub(super) local_truncated: bool,
    /// Drawn graph. Replace it only through [`GraphViewState::set_ui_graph`].
    pub(super) ui_graph: Option<UiGraph>,
    /// Draw data for `ui_graph` under the current `positions`.
    pub(super) scene: Option<GraphScene>,
    pub(super) positions: PosCache,
    pub(super) layout_ready: bool,
    pub(super) need_fit: bool,
    /// Banner after expand hit max_nodes or added nothing.
    pub(super) expand_note: Option<String>,
    /// True when `local_view` contains Expand merges beyond the plain seed BFS
    /// (gates the "Reset to seed" confirmation).
    pub(super) expanded_dirty: bool,
    /// "Reset to seed" confirmation popup is open.
    pub(super) confirm_reset: bool,

    pub(super) selected: Option<String>,
    /// Full wiki/raw body for the selected node (HTTP or --db).
    pub(super) content: Option<DocumentBody>,
    pub(super) content_error: Option<String>,
    pub(super) pan: Vec2,
    pub(super) zoom: f32,
}

impl GraphViewState {
    /// Initial view for the CLI arguments (seed, depth and node budget clamped).
    pub(super) fn new(open: &OpenArgs) -> Self {
        Self {
            full_view: None,
            full_view_rev: 0,
            seed_suggestions_cache: None,
            seed_search_keys: None,
            empty_stats_cache: std::cell::RefCell::new(None),
            wiki_target_cache: std::cell::RefCell::new(None),
            load_error: None,
            raw_truncated: false,
            raw_node_count: 0,
            seed_input: open.seed.clone().unwrap_or_default(),
            seed_id: None,
            seed_error: None,
            depth: open.depth.clamp(1, 3),
            max_nodes: open.max_nodes.clamp(1, UI_GRAPH_EXPORT_MAX_NODES),
            lens: GraphLens::Neighborhood,
            show_tags: false,
            show_stubs: true,
            prev_show_tags: false,
            prev_show_stubs: true,
            local_view: None,
            local_truncated: false,
            ui_graph: None,
            scene: None,
            positions: PosCache::default(),
            layout_ready: false,
            need_fit: true,
            expand_note: None,
            expanded_dirty: false,
            confirm_reset: false,
            selected: None,
            content: None,
            content_error: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }

    /// Show `graph` (already laid out in `positions`) or nothing, keeping the
    /// per-graph draw data in step with it.
    pub(super) fn set_ui_graph(&mut self, graph: Option<UiGraph>) {
        self.scene = graph
            .as_ref()
            .map(|graph| GraphScene::build(graph, &self.positions));
        self.ui_graph = graph;
    }
}
