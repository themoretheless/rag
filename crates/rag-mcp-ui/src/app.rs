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
use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use crate::load::{
    local_neighbors, resolve_seed, sort_wiki_pages, CliSource, DocumentBody, GatewayHealth,
    GraphSourceKind, LoadedGraph, OpenArgs, WikiPageMeta, WikiPutRequest, UI_HARD_MAX_NODES,
};
use crate::ui::canvas::draw_canvas;
use crate::ui::detail::{draw_detail, DetailAction};
use crate::ui::empty::{
    draw_empty_banner, draw_no_source, EmptyGraphStats, EmptyKind, NoSourceAction,
};
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

fn project_options(view: Option<&GraphView>) -> Vec<String> {
    view.into_iter().flat_map(|graph| &graph.nodes)
        .filter_map(|node| serde_json::from_str::<serde_json::Value>(&node.metadata_json).ok())
        .filter_map(|meta| meta.get("wing").and_then(|value| value.as_str()).map(str::to_owned))
        .filter(|project| !project.trim().is_empty())
        .collect::<BTreeSet<_>>().into_iter().collect()
}

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
    /// HTTP base URL editable on the no-source start screen.
    connect_url: String,

    full_view: Option<GraphView>,
    source: Option<GraphSourceKind>,
    load_error: Option<String>,
    raw_truncated: bool,
    raw_node_count: usize,
    ops_health: Option<GatewayHealth>,

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
            connect_url: "http://127.0.0.1:7432".into(),
            full_view: None,
            source: None,
            load_error: None,
            raw_truncated: false,
            raw_node_count: 0,
            ops_health: None,
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
            filter_wing: String::new(),
            filter_room: String::new(),
            prev_filter_wing: String::new(),
            prev_filter_room: String::new(),
            local_view: None,
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
    }

    /// Kick the initial / retry topology load on the worker (no-op without a source).
    fn dispatch_graph_load(&mut self) {
        let Some(src) = self.open.source.clone() else {
            return;
        };
        let seq = self.next_seq();
        self.pending_graph = Some(seq);
        self.worker.send(WorkerCmd::LoadGraph {
            seq,
            source: src,
            seed: self.open.seed.clone(),
            depth: self.depth,
        });
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
        self.wiki_loaded = false;
        self.dispatch_graph_load();
    }

    fn apply_loaded(&mut self, loaded: LoadedGraph) {
        self.load_error = None;
        self.raw_truncated = loaded.truncated;
        self.raw_node_count = loaded.raw_node_count;
        self.ops_health = loaded.health;
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
        match self.source.as_ref() {
            Some(s) => match LoadSource::from_graph_source(s) {
                Some(source) => {
                    let seq = self.next_seq();
                    self.pending_catalog = Some(seq);
                    self.worker.send(WorkerCmd::LoadWikiCatalog { seq, source });
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
        // Any in-flight Expand merges old topology into the new local view:
        // invalidate it so the late answer is dropped by seq.
        self.pending_expand = None;
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
        let full = self.full_view.clone();
        if db_path.is_none() && full.is_none() {
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
                            // Catalog may still answer (or fail with a clearer wiki error).
                            self.ensure_wiki_loaded();
                        }
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
                            self.auto_open_initial_page();
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
        if ctx.wants_keyboard_input() {
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_worker_events();
        self.handle_hotkeys(ctx);
        let project_options = project_options(self.full_view.as_ref());

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
                        let seed_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.seed_input)
                                .desired_width(200.0)
                                .hint_text("id / label / document_id"),
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
                            egui::popup::popup_below_widget(
                                ui,
                                ui.make_persistent_id("seed_suggestions"),
                                &seed_resp,
                                egui::PopupCloseBehavior::CloseOnClick,
                                |ui| {
                                    for (id, label) in &suggestions {
                                        if ui
                                            .selectable_label(false, format!("{label} · {id}"))
                                            .clicked()
                                        {
                                            picked = Some(id.clone());
                                        }
                                    }
                                },
                            );
                        }
                        let first_pick = suggestions.first().map(|(id, _)| id.clone());
                        drop(suggestions);
                        if seed_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            // Enter: exact input, or first suggestion when one matches.
                            if let Some(id) = first_pick {
                                self.seed_input = id;
                            }
                            self.apply_seed_from_input();
                        } else if let Some(id) = picked {
                            seed_resp.surrender_focus();
                            self.seed_input = id;
                            self.apply_seed_from_input();
                        }
                        if ui.button("Apply seed").clicked() {
                            self.apply_seed_from_input();
                        }
                        ui.separator();
                        ui.label("depth");
                        let mut d = self.depth as i32;
                        if ui.add(egui::DragValue::new(&mut d).range(1..=3)).changed() {
                            self.depth = d as u32;
                            self.rebuild_ui_graph();
                        }
                        ui.checkbox(&mut self.show_tags, "tags");
                        ui.checkbox(&mut self.show_stubs, "stubs");
                        ui.separator();
                        ui.label("project");
                        egui::ComboBox::from_id_salt("project_filter")
                            .selected_text(if self.filter_wing.is_empty() { "All projects" } else { &self.filter_wing })
                            .width(120.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.filter_wing, String::new(), "All projects");
                                for project in &project_options {
                                    ui.selectable_value(&mut self.filter_wing, project.clone(), project);
                                }
                            });
                        ui.label("room");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.filter_room)
                                .desired_width(80.0)
                                .hint_text("all rooms"),
                        );
                        if ui.button("Clear filters").clicked() {
                            self.filter_wing.clear();
                            self.filter_room.clear();
                        }
                        if ui
                            .button("Reset to seed")
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
                                egui::Button::new("Expand neighbors"),
                            )
                            .clicked()
                        {
                            self.expand_selected();
                        }
                        if expanding {
                            ui.spinner();
                        }
                        if ui
                            .add_enabled(self.selected.is_some(), egui::Button::new("Open as wiki"))
                            .on_hover_text("Open selected node as article")
                            .clicked()
                        {
                            self.open_selected_graph_node_in_wiki();
                        }
                        if ui
                            .add_enabled(
                                self.selected.is_some() && self.pending_content.is_none(),
                                egui::Button::new("Read content"),
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

        if self.show_tags != self.prev_show_tags
            || self.show_stubs != self.prev_show_stubs
            || self.filter_wing != self.prev_filter_wing
            || self.filter_room != self.prev_filter_room
        {
            self.prev_show_tags = self.show_tags;
            self.prev_show_stubs = self.show_stubs;
            self.prev_filter_wing = self.filter_wing.clone();
            self.prev_filter_room = self.filter_room.clone();
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
                        if self.any_pending() {
                            ui.spinner();
                        }
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
                                ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "unsaved");
                            }
                        }
                        if let Some(note) = &self.wiki_save_note {
                            ui.separator();
                            ui.colored_label(egui::Color32::from_rgb(100, 180, 120), note);
                        }
                        ui.separator();
                        ui.weak("Dual pane · A/B focus · Edit · [[wikilinks]] · sidebar · Reload");
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
            ViewMode::Wiki => {
                // Left: catalog (polished dual-pane nav column).
                if self.wiki_sidebar_visible {
                    egui::SidePanel::left("wiki_nav")
                        .default_width(250.0)
                        .width_range(190.0..=420.0)
                        .resizable(true)
                        .show_separator_line(false)
                        .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(12.0))
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
                                &mut self.wiki_show_summaries,
                            ) {
                                self.open_wiki_page_id(&id);
                            }
                        });
                }

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
                    egui::SidePanel::right("wiki_info")
                        .default_width(260.0)
                        .width_range(220.0..=380.0)
                        .resizable(true)
                        .show_separator_line(false)
                        .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(14.0))
                        .show(ctx, |ui| {
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

                egui::CentralPanel::default().show(ctx, |ui| {
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

                egui::CentralPanel::default().show(ctx, |ui| {
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
                .show(ctx, |ui| {
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
