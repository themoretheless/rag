//! GraphApp: eframe::App - graph inspector + Obsidian/Notion-style wiki browser.

use egui::Vec2;
use rag_mcp::GraphView;

use crate::adapter::{adapt, topology_generation, AdaptOptions, UiGraph};
use crate::layout::{place_missing_near_neighbors, radial_place, PosCache};
use std::collections::HashSet;

use crate::load::{
    expand_neighbors_local, expand_neighbors_store, fetch_backlinks_http, fetch_document_db,
    fetch_document_http, fetch_wiki_list_db, fetch_wiki_list_http, load_http, load_live_db,
    load_snapshot_path, local_neighbors, put_wiki_http, resolve_seed, save_wiki_db, BacklinkItem,
    CliSource, DocumentBody, GraphSourceKind, LoadedGraph, OpenArgs, UI_HARD_MAX_NODES,
    WikiPageMeta, WikiPutRequest,
};
use crate::ui::canvas::draw_canvas;
use crate::ui::detail::{draw_detail, DetailAction};
use crate::ui::empty::{draw_empty_banner, EmptyGraphStats, EmptyKind};
use crate::ui::status::draw_status;
use crate::ui::wiki::{
    content_summary_line, draw_wiki_edit_view, draw_wiki_read_view, draw_wiki_sidebar,
    slug_from_wiki_uri, WikiEditBuffers,
};

/// Top-level UI mode (graph topology vs wiki articles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ViewMode {
    #[default]
    Graph,
    Wiki,
}

/// Which wiki article column is focused (sidebar / links land here when dual-pane is on).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum WikiPane {
    #[default]
    A,
    B,
}

pub struct GraphApp {
    open: OpenArgs,
    full_view: Option<GraphView>,
    source: Option<GraphSourceKind>,
    load_error: Option<String>,
    raw_truncated: bool,
    raw_node_count: usize,

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

    /// Local topology (seed BFS and/or Expand merges) before adapter filters.
    local_view: Option<GraphView>,
    ui_graph: Option<UiGraph>,
    positions: PosCache,
    layout_ready: bool,
    need_fit: bool,
    last_topo: u64,
    /// Banner after expand hit max_nodes or added nothing.
    expand_note: Option<String>,

    selected: Option<String>,
    /// Full wiki/raw body for the selected node (HTTP or --db).
    content: Option<DocumentBody>,
    content_error: Option<String>,
    pan: Vec2,
    zoom: f32,

    // --- Wiki browser state ---
    wiki_pages: Vec<WikiPageMeta>,
    wiki_filter: String,
    wiki_selected_id: Option<String>,
    wiki_article: Option<DocumentBody>,
    wiki_error: Option<String>,
    wiki_loaded: bool,
    /// History stack of wiki page ids for Back (Obsidian-like).
    wiki_history: Vec<String>,
    wiki_backlinks: Vec<BacklinkItem>,
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
    wiki_backlinks_b: Vec<BacklinkItem>,
}

impl GraphApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, open: OpenArgs) -> Self {
        let depth = open.depth.clamp(1, 3);
        let max_nodes = open.max_nodes.clamp(1, UI_HARD_MAX_NODES as u32);
        let seed_input = open.seed.clone().unwrap_or_default();
        let mut app = Self {
            open,
            full_view: None,
            source: None,
            load_error: None,
            raw_truncated: false,
            raw_node_count: 0,
            mode: ViewMode::Wiki,
            seed_input,
            seed_id: None,
            seed_error: None,
            depth,
            max_nodes,
            show_tags: false,
            show_stubs: true,
            prev_show_tags: false,
            prev_show_stubs: true,
            local_view: None,
            ui_graph: None,
            positions: PosCache::default(),
            layout_ready: false,
            need_fit: true,
            last_topo: 0,
            expand_note: None,
            selected: None,
            content: None,
            content_error: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            wiki_pages: Vec::new(),
            wiki_filter: String::new(),
            wiki_selected_id: None,
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
        app.try_initial_load();
        app
    }

    fn try_initial_load(&mut self) {
        let Some(src) = self.open.source.clone() else {
            return;
        };
        match src {
            CliSource::Snapshot(path) => match load_snapshot_path(&path) {
                Ok(loaded) => self.apply_loaded(loaded),
                Err(e) => self.load_error = Some(e),
            },
            CliSource::Db(path) => {
                match load_live_db(&path, self.open.seed.as_deref(), self.depth) {
                    Ok(loaded) => self.apply_loaded(loaded),
                    Err(e) => self.load_error = Some(e),
                }
            }
            CliSource::Http(base) => {
                match load_http(&base, self.open.seed.as_deref(), self.depth) {
                    Ok(loaded) => self.apply_loaded(loaded),
                    Err(e) => self.load_error = Some(e),
                }
            }
        }
    }

    fn apply_loaded(&mut self, loaded: LoadedGraph) {
        self.load_error = None;
        self.raw_truncated = loaded.truncated;
        self.raw_node_count = loaded.raw_node_count;
        self.source = Some(loaded.source);
        self.full_view = Some(loaded.view);
        if !self.seed_input.is_empty() {
            self.apply_seed_from_input();
        } else {
            self.rebuild_ui_graph();
        }
        // Prefer wiki browser when source can load pages.
        self.ensure_wiki_loaded();
    }

    fn ensure_wiki_loaded(&mut self) {
        if self.wiki_loaded {
            return;
        }
        self.reload_wiki_catalog();
    }

    fn reload_wiki_catalog(&mut self) {
        self.wiki_error = None;
        let result = match self.source.as_ref() {
            Some(GraphSourceKind::HttpService { base }) => fetch_wiki_list_http(base),
            Some(GraphSourceKind::LiveStore { path }) => fetch_wiki_list_db(path),
            Some(GraphSourceKind::SnapshotFile { .. } | GraphSourceKind::VaultGraphJson { .. }) => {
                Err("snapshot mode has no wiki catalog; use --http or --db".into())
            }
            None => Err("no data source".into()),
        };
        match result {
            Ok(pages) => {
                self.wiki_pages = pages;
                self.wiki_loaded = true;
                // Open seed title if it matches a wiki page.
                if self.wiki_selected_id.is_none() && !self.seed_input.trim().is_empty() {
                    let q = self.seed_input.trim();
                    if let Some(p) = self.wiki_pages.iter().find(|p| {
                        p.title.eq_ignore_ascii_case(q)
                            || p.slug.eq_ignore_ascii_case(q)
                            || p.id == q
                    }) {
                        let id = p.id.clone();
                        self.open_wiki_page_id(&id);
                    }
                } else if self.wiki_selected_id.is_none() {
                    // Default: first page (often hub / overview).
                    if let Some(p) = self
                        .wiki_pages
                        .iter()
                        .find(|p| p.title.to_lowercase().contains("обзор") || p.slug.contains("overview"))
                        .or_else(|| self.wiki_pages.first())
                    {
                        let id = p.id.clone();
                        self.open_wiki_page_id(&id);
                    }
                }
            }
            Err(e) => {
                self.wiki_error = Some(e);
                self.wiki_loaded = true;
            }
        }
    }

    fn open_wiki_page_id(&mut self, id: &str) {
        if self.wiki_dual_pane && self.wiki_focus == WikiPane::B {
            self.open_wiki_page_in_b(id);
            return;
        }
        if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some(
                "unsaved edits: Save or Cancel before opening another page".into(),
            );
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
        self.wiki_article = None;
        self.wiki_error = None;
        self.wiki_edit = None;
        self.wiki_save_note = None;
        match self.fetch_wiki_body(id) {
            Ok(body) => {
                self.wiki_backlinks = self.load_backlinks(&body.id);
                self.wiki_article = Some(body);
            }
            Err(e) => {
                self.wiki_backlinks.clear();
                self.wiki_error = Some(e);
            }
        }
    }

    /// Load a page into the right (secondary) dual-pane column. No history / edit.
    fn open_wiki_page_in_b(&mut self, id: &str) {
        self.wiki_selected_id_b = Some(id.to_string());
        self.wiki_article_b = None;
        self.wiki_error_b = None;
        match self.fetch_wiki_body(id) {
            Ok(body) => {
                self.wiki_backlinks_b = self.load_backlinks(&body.id);
                self.wiki_article_b = Some(body);
            }
            Err(e) => {
                self.wiki_backlinks_b.clear();
                self.wiki_error_b = Some(e);
            }
        }
    }

    fn clear_wiki_pane_b(&mut self) {
        self.wiki_selected_id_b = None;
        self.wiki_article_b = None;
        self.wiki_error_b = None;
        self.wiki_backlinks_b.clear();
    }

    fn fetch_wiki_body(&self, id: &str) -> Result<DocumentBody, String> {
        let meta = self.wiki_pages.iter().find(|p| p.id == id).cloned();
        let Some(meta) = meta else {
            return Err(format!("page id {id} not in catalog"));
        };
        match self.source.as_ref() {
            Some(GraphSourceKind::HttpService { base }) => {
                fetch_document_http(base, Some(&meta.id), Some(&meta.uri), Some(&meta.title))
            }
            Some(GraphSourceKind::LiveStore { path }) => {
                fetch_document_db(path, Some(&meta.id), Some(&meta.uri))
            }
            _ => Err("wiki articles require --http or --db".into()),
        }
    }

    fn load_backlinks(&self, document_id: &str) -> Vec<BacklinkItem> {
        if let Some(GraphSourceKind::HttpService { base }) = self.source.as_ref() {
            if let Ok(bl) = fetch_backlinks_http(base, document_id) {
                return bl;
            }
        } else if let Some(GraphSourceKind::LiveStore { path }) = self.source.as_ref() {
            if let Ok(store) = rag_mcp::Store::open(path) {
                if let Ok(rows) = store.wiki_backlinks_for_document(document_id) {
                    return rows
                        .into_iter()
                        .map(|(label, id)| BacklinkItem { label, id })
                        .collect();
                }
            }
        }
        Vec::new()
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

    fn save_wiki_edit(&mut self) {
        let Some(edit) = self.wiki_edit.as_ref() else {
            return;
        };
        let Some(art) = self.wiki_article.as_ref() else {
            self.wiki_error = Some("no page open to save".into());
            return;
        };
        let id = art.id.clone();
        let uri = art.uri.clone();
        let title = edit.title.clone();
        let content = edit.content.clone();
        let if_match_revision = edit.base_revision;
        let if_match_etag = edit.base_etag.clone();

        let slug = slug_from_wiki_uri(&uri);
        let result = match self.source.as_ref() {
            Some(GraphSourceKind::HttpService { base }) => put_wiki_http(
                base,
                &WikiPutRequest {
                    id: id.clone(),
                    slug,
                    uri: Some(uri.clone()),
                    title: title.clone(),
                    content: content.clone(),
                    if_match_revision,
                    if_match_etag,
                },
            ),
            Some(GraphSourceKind::LiveStore { path }) => {
                save_wiki_db(path, &id, &title, &content, if_match_revision)
            }
            _ => Err("wiki save requires --http or --db".into()),
        };

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
                self.wiki_pages.sort_by(|a, b| a.title.cmp(&b.title));
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
        self.wiki_backlinks = self.load_backlinks(document_id);
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
            self.wiki_error = Some(
                "unsaved edits: Save or Cancel before going back".into(),
            );
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
            self.wiki_error = Some(
                "unsaved edits: Save or Cancel before following a link".into(),
            );
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
        // Exact wiki uri only (no fuzzy label pick - avoids wrong page).
        let result = match self.source.as_ref() {
            Some(GraphSourceKind::HttpService { base }) => {
                fetch_document_http(base, None, Some(&format!("wiki://{q}")), None).or_else(|_| {
                    fetch_document_http(base, None, Some(q), None)
                })
            }
            Some(GraphSourceKind::LiveStore { path }) => {
                fetch_document_db(path, None, Some(&format!("wiki://{q}"))).or_else(|_| {
                    fetch_document_db(path, None, Some(q))
                })
            }
            _ => Err("no source".into()),
        };
        match result {
            Ok(body) => {
                if body.layer != "wiki" && !body.uri.starts_with("wiki://") {
                    // Allow raw docs linked by exact title from graph, still open.
                }
                let id = body.id.clone();
                if !self.wiki_pages.iter().any(|p| p.id == id) {
                    let slug = body
                        .uri
                        .strip_prefix("wiki://")
                        .unwrap_or(body.uri.as_str())
                        .to_string();
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
                    self.wiki_pages.sort_by(|a, b| a.title.cmp(&b.title));
                }
                self.open_wiki_page_id(&id);
                // open_wiki_page_id reloads; set article from body to avoid double fetch miss
                if into_b {
                    if self.wiki_article_b.is_none() {
                        self.wiki_article_b = Some(body);
                    }
                    self.wiki_error_b = None;
                } else {
                    if self.wiki_article.is_none() {
                        self.wiki_article = Some(body);
                    }
                    self.wiki_error = None;
                }
            }
            Err(_) => {
                let msg = format!(
                    "unresolved link [[{q}]] - no page with that title/slug (create via write_wiki_page)"
                );
                if into_b {
                    self.wiki_error_b = Some(msg);
                } else {
                    self.wiki_error = Some(msg);
                }
            }
        }
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
                self.ui_graph = None;
                self.layout_ready = false;
            }
        }
    }

    /// Load full document body for the current selection (wiki or raw).
    fn load_content_for_selected(&mut self) {
        self.content = None;
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

        let doc_id = node.document_id.as_deref();
        let uri = node.uri.as_deref();
        let label = node.label.as_str();

        let result = match self.source.as_ref() {
            Some(GraphSourceKind::HttpService { base }) => {
                fetch_document_http(base, doc_id, uri, Some(label))
            }
            Some(GraphSourceKind::LiveStore { path }) => fetch_document_db(path, doc_id, uri),
            Some(GraphSourceKind::SnapshotFile { .. } | GraphSourceKind::VaultGraphJson { .. }) => {
                Err(
                    "snapshot mode has no document bodies; use --http or --db".into(),
                )
            }
            None => Err("no data source".into()),
        };

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

    fn rebuild_ui_graph(&mut self) {
        self.expand_note = None;
        let Some(full) = self.full_view.as_ref() else {
            self.local_view = None;
            self.ui_graph = None;
            self.layout_ready = false;
            return;
        };

        let Some(seed) = self.seed_id.as_deref() else {
            self.local_view = None;
            self.ui_graph = Some(UiGraph::default());
            self.layout_ready = false;
            return;
        };

        let local = local_neighbors(full, seed, self.depth, self.max_nodes as usize);
        self.apply_local_topology(local, true);
    }

    /// Adapt `local`, update layout. `reset_layout` re-runs RadialLocal; expand uses false.
    fn apply_local_topology(&mut self, local: GraphView, reset_layout: bool) {
        let seed = self.seed_id.clone();
        let opts = AdaptOptions {
            seed_id: seed.clone(),
            show_tags: self.show_tags,
            show_stubs: self.show_stubs,
            pkb_rels_only: true,
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
    /// into the local view under `max_nodes`.
    ///
    /// - Snapshot / vault: client BFS on the loaded `full_view`.
    /// - `--db`: `Store::neighbors` (exclusive re-open), then merge; cross-edges
    ///   filled from the in-memory export when available.
    /// Does not reseed the local view (EGUI_GRAPH_VIEW §5.1 / §6.1).
    fn expand_selected(&mut self) {
        let Some(sel) = self.selected.clone() else {
            return;
        };
        self.expand_note = None;
        self.seed_error = None;

        // No seed yet: selected becomes seed at depth 1, then paint.
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
                "Expand blocked: already at max_nodes ({max_n})"
            ));
            return;
        }

        let before = current.nodes.len();
        let merged = match self.open.source.clone() {
            Some(CliSource::Db(path)) => {
                match expand_neighbors_store(
                    &path,
                    &current,
                    &sel,
                    self.max_nodes,
                    self.full_view.as_ref(),
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        self.expand_note = Some(e);
                        return;
                    }
                }
            }
            _ => {
                let Some(full) = self.full_view.as_ref() else {
                    self.expand_note = Some("Expand requires a loaded graph".into());
                    return;
                };
                expand_neighbors_local(full, &current, &sel, max_n)
            }
        };

        let after = merged.nodes.len();
        if after == before {
            self.expand_note = Some(format!(
                "No new neighbors for “{sel}” within max_nodes ({max_n})"
            ));
            // Still refresh layout in case edges appeared without new nodes (unlikely).
        } else if after >= max_n {
            self.expand_note = Some(format!(
                "Expand filled view to max_nodes ({max_n}); some neighbors may be omitted"
            ));
        }

        // Status depth: at least selected.depth + 1 (one hop outward), capped at 3.
        if let Some(g) = &self.ui_graph {
            if let Some(n) = g.nodes.iter().find(|n| n.id == sel) {
                self.depth = self.depth.max((n.depth + 1).min(3));
            }
        } else {
            self.depth = self.depth.max(1).min(3);
        }

        self.apply_local_topology(merged, /*reset_layout=*/ false);
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
                            "raw nodes {} > cap {}",
                            self.raw_node_count, UI_HARD_MAX_NODES
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
}

impl eframe::App for GraphApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("rag-mcp-ui");
                ui.separator();
                // Mode tabs (Notion/Obsidian-like)
                if ui
                    .selectable_label(self.mode == ViewMode::Wiki, "Wiki")
                    .on_hover_text("Articles / notes (Obsidian-style)")
                    .clicked()
                {
                    self.mode = ViewMode::Wiki;
                    self.ensure_wiki_loaded();
                }
                if ui
                    .selectable_label(self.mode == ViewMode::Graph, "Graph")
                    .on_hover_text("Local object graph")
                    .clicked()
                {
                    self.mode = ViewMode::Graph;
                }
                ui.separator();

                match self.mode {
                    ViewMode::Wiki => {
                        if ui
                            .add_enabled(!self.wiki_history.is_empty(), egui::Button::new("← Back"))
                            .clicked()
                        {
                            self.wiki_go_back();
                        }
                        if ui.button("Reload wiki").clicked() {
                            if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
                                self.wiki_error = Some(
                                    "unsaved edits: Save or Cancel before reload".into(),
                                );
                            } else {
                                self.wiki_edit = None;
                                self.wiki_loaded = false;
                                self.reload_wiki_catalog();
                            }
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
                            if ui
                                .add_enabled(self.wiki_can_write(), egui::Button::new("Save"))
                                .clicked()
                            {
                                self.save_wiki_edit();
                            }
                            if ui.button("Cancel edit").clicked() {
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
                                self.seed_input = id;
                                self.mode = ViewMode::Graph;
                                self.apply_seed_from_input();
                                if self.seed_error.is_some() {
                                    self.seed_input = title;
                                    self.apply_seed_from_input();
                                }
                            }
                        }
                    }
                    ViewMode::Graph => {
                        ui.label("seed");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.seed_input)
                                .desired_width(200.0)
                                .hint_text("id / label / document_id"),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.apply_seed_from_input();
                        }
                        if ui.button("Apply seed").clicked() {
                            self.apply_seed_from_input();
                        }
                        ui.separator();
                        ui.label("depth");
                        let mut d = self.depth as i32;
                        if ui
                            .add(egui::DragValue::new(&mut d).range(1..=3))
                            .changed()
                        {
                            self.depth = d as u32;
                            self.rebuild_ui_graph();
                        }
                        ui.checkbox(&mut self.show_tags, "tags");
                        ui.checkbox(&mut self.show_stubs, "stubs");
                        if ui.button("Rebuild").clicked() {
                            self.rebuild_ui_graph();
                        }
                        if ui
                            .add_enabled(
                                self.selected.is_some(),
                                egui::Button::new("Expand neighbors"),
                            )
                            .clicked()
                        {
                            self.expand_selected();
                        }
                        if ui
                            .add_enabled(
                                self.selected.is_some(),
                                egui::Button::new("Open as wiki"),
                            )
                            .on_hover_text("Open selected node as article")
                            .clicked()
                        {
                            self.open_selected_graph_node_in_wiki();
                        }
                        if ui
                            .add_enabled(
                                self.selected.is_some(),
                                egui::Button::new("Read content"),
                            )
                            .clicked()
                        {
                            self.load_content_for_selected();
                        }
                    }
                }
            });
        });

        if self.show_tags != self.prev_show_tags || self.show_stubs != self.prev_show_stubs {
            self.prev_show_tags = self.show_tags;
            self.prev_show_stubs = self.show_stubs;
            self.rebuild_ui_graph();
        }

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            match self.mode {
                ViewMode::Wiki => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("mode=wiki");
                        if let Some(s) = self.source.as_ref() {
                            ui.label(format!("source={}", s.label()));
                        }
                        ui.separator();
                        ui.label(format!("pages={}", self.wiki_pages.len()));
                        if self.wiki_dual_pane {
                            ui.separator();
                            ui.label(format!(
                                "dual focus={}",
                                match self.wiki_focus {
                                    WikiPane::A => "A",
                                    WikiPane::B => "B",
                                }
                            ));
                        }
                        if let Some(art) = &self.wiki_article {
                            ui.separator();
                            ui.label(format!("A={}", art.title));
                            if let Some(r) = art.revision {
                                ui.weak(format!("r{r}"));
                            }
                        }
                        if let Some(art) = &self.wiki_article_b {
                            ui.separator();
                            ui.label(format!("B={}", art.title));
                        }
                        if self.wiki_edit.is_some() {
                            ui.separator();
                            ui.strong("editing");
                            if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
                                ui.colored_label(
                                    egui::Color32::from_rgb(220, 160, 60),
                                    "unsaved",
                                );
                            }
                        }
                        if let Some(note) = &self.wiki_save_note {
                            ui.separator();
                            ui.colored_label(egui::Color32::from_rgb(100, 180, 120), note);
                        }
                        ui.separator();
                        ui.weak(
                            "Dual pane · A/B focus · Edit · [[wikilinks]] · sidebar · Reload",
                        );
                    });
                }
                ViewMode::Graph => {
                    let seed_label = self.seed_id.as_deref().or_else(|| {
                        if self.seed_input.is_empty() {
                            None
                        } else {
                            Some(self.seed_input.as_str())
                        }
                    });
                    let truncated = self
                        .ui_graph
                        .as_ref()
                        .is_some_and(|g| g.truncated_nodes || g.truncated_edges)
                        || self.raw_truncated;
                    draw_status(
                        ui,
                        self.source.as_ref(),
                        seed_label,
                        self.depth,
                        self.ui_graph.as_ref(),
                        self.layout_ready,
                        truncated,
                        self.status_banner().as_deref(),
                    );
                }
            }
        });

        match self.mode {
            ViewMode::Wiki => {
                // Left: catalog (polished dual-pane nav column).
                egui::SidePanel::left("wiki_nav")
                    .default_width(280.0)
                    .width_range(200.0..=480.0)
                    .resizable(true)
                    .show_separator_line(true)
                    .show(ctx, |ui| {
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
                        ) {
                            self.open_wiki_page_id(&id);
                        }
                    });

                // Right: secondary article when dual-pane is on.
                if self.wiki_dual_pane {
                    egui::SidePanel::right("wiki_pane_b")
                        .default_width(420.0)
                        .width_range(280.0..=900.0)
                        .resizable(true)
                        .show_separator_line(true)
                        .show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                let focused = self.wiki_focus == WikiPane::B;
                                if ui
                                    .selectable_label(focused, "Pane B")
                                    .on_hover_text("Secondary article (read-only)")
                                    .clicked()
                                {
                                    self.wiki_focus = WikiPane::B;
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
                                &titles,
                                &slugs,
                                &self.wiki_backlinks_b,
                                false,
                            );
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

                egui::CentralPanel::default().show(ctx, |ui| {
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
                            )
                        };
                        if action.save {
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
                            &titles,
                            &slugs,
                            &self.wiki_backlinks,
                            can_write,
                        );
                        if action.start_edit {
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
                    egui::SidePanel::right("detail")
                        .default_width(detail_w)
                        .show(ctx, |ui| {
                            if let (Some(sel), Some(g)) =
                                (self.selected.as_deref(), self.ui_graph.as_ref())
                            {
                                let action = draw_detail(
                                    ui,
                                    g,
                                    sel,
                                    self.content.as_ref(),
                                    self.content_error.as_deref(),
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

                egui::CentralPanel::default().show(ctx, |ui| {
                    if let Some((kind, detail)) = self.empty_kind() {
                        let stats = self.empty_stats();
                        draw_empty_banner(ui, kind, detail.as_deref(), stats.as_ref());
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
                        }
                        self.selected = Some(id);
                    } else if out.clicked_empty {
                        self.selected = None;
                        self.content = None;
                        self.content_error = None;
                    }
                });
            }
        }
    }
}
