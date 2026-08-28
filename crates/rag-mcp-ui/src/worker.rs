//! Background worker thread: all blocking IO (HTTP / DuckDB) off the UI thread.
//!
//! EGUI_GRAPH_VIEW §2.5 / §8.3: the egui frame thread must never touch DuckDB or
//! blocking network. `GraphApp` sends [`WorkerCmd`] values carrying everything the
//! job needs (sources are cheap clones; views are ≤ UI hard cap), the worker runs
//! them serially and answers with [`WorkerEvt`]. Each job carries a `seq`; the app
//! ignores events whose `seq` no longer matches the pending slot (stale answers
//! after a newer request was issued).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use rag_mcp::GraphView;

use crate::load::{
    expand_neighbors_local, expand_neighbors_store, fetch_backlinks_http, fetch_document_db,
    fetch_document_http, fetch_wiki_list_db, fetch_wiki_list_http, load_cli_source, put_wiki_http,
    save_wiki_db, BacklinkItem, CliSource, DocumentBody, GraphSourceKind, LoadedGraph,
    WikiPageMeta, WikiPutRequest,
};

/// Read/write-capable source for document/wiki jobs (HTTP gateway or exclusive DB).
#[derive(Debug, Clone)]
pub enum LoadSource {
    Http(String),
    Db(PathBuf),
}

impl LoadSource {
    /// Derive from the loaded graph source; snapshots cannot serve documents/wiki.
    pub fn from_graph_source(source: &GraphSourceKind) -> Option<Self> {
        match source {
            GraphSourceKind::HttpService { base } => Some(Self::Http(base.clone())),
            GraphSourceKind::LiveStore { path } => Some(Self::Db(path.clone())),
            GraphSourceKind::SnapshotFile { .. } | GraphSourceKind::VaultGraphJson { .. } => None,
        }
    }
}

/// Jobs dispatched from the UI thread to the worker.
#[derive(Debug)]
pub enum WorkerCmd {
    /// Initial / retry topology load (snapshot, exclusive db, or http export).
    LoadGraph {
        seq: u64,
        source: CliSource,
        seed: Option<String>,
        depth: u32,
    },
    /// Wiki catalog list for the sidebar.
    LoadWikiCatalog { seq: u64, source: LoadSource },
    /// Open a wiki page body + its backlinks.
    ///
    /// `meta` set: fetch by catalog id/uri. `meta` None + `q`: unresolved
    /// `[[link]]` fallback by exact wiki uri. `push_history` mirrors the
    /// Obsidian-style Back stack for the fallback path.
    OpenPage {
        seq: u64,
        pane_b: bool,
        push_history: bool,
        meta: Option<WikiPageMeta>,
        q: Option<String>,
        source: LoadSource,
    },
    /// Backlinks only (e.g. refresh after save).
    LoadBacklinks {
        seq: u64,
        document_id: String,
        source: LoadSource,
    },
    /// Full document body for a selected graph node ("Read content").
    ReadContent {
        seq: u64,
        node_id: String,
        doc_id: Option<String>,
        uri: Option<String>,
        label: String,
        source: LoadSource,
    },
    /// One-hop neighbor expansion of `selected`, merged into `current`.
    ///
    /// Client-side BFS on `full` for snapshot/http; `Store::neighbors` when
    /// `db_path` is set (exclusive re-open, dual-live still forbidden).
    ExpandNeighbors {
        seq: u64,
        selected: String,
        current: GraphView,
        full: Option<GraphView>,
        db_path: Option<PathBuf>,
        max_nodes: u32,
    },
    /// Save a wiki page (HTTP PUT /v1/wiki or exclusive `--db` write).
    ///
    /// `req` carries the CAS fields (If-Match revision/etag); a 409 / revision
    /// conflict comes back as `Err` with a "conflict" message so the edit view
    /// can offer Reload without dropping the user's buffers.
    SavePage {
        seq: u64,
        req: WikiPutRequest,
        source: LoadSource,
    },
}

/// Results delivered back to the UI thread.
#[derive(Debug)]
pub enum WorkerEvt {
    GraphLoaded {
        seq: u64,
        result: Result<LoadedGraph, String>,
    },
    WikiCatalog {
        seq: u64,
        result: Result<Vec<WikiPageMeta>, String>,
    },
    PageOpened {
        seq: u64,
        pane_b: bool,
        push_history: bool,
        /// Original `[[link]]` query for the unresolved-link error message.
        q: Option<String>,
        result: Result<(DocumentBody, Vec<BacklinkItem>), String>,
    },
    Backlinks {
        seq: u64,
        document_id: String,
        result: Result<Vec<BacklinkItem>, String>,
    },
    Content {
        seq: u64,
        node_id: String,
        result: Result<DocumentBody, String>,
    },
    Expanded {
        seq: u64,
        selected: String,
        result: Result<GraphView, String>,
    },
    SavedPage {
        seq: u64,
        result: Result<DocumentBody, String>,
    },
}

/// Channel endpoints owned by `GraphApp`.
pub struct WorkerHandle {
    tx: Sender<WorkerCmd>,
    pub rx: Receiver<WorkerEvt>,
}

impl WorkerHandle {
    pub fn send(&self, cmd: WorkerCmd) {
        // A dead worker means the window is going away; ignore send errors.
        let _ = self.tx.send(cmd);
    }
}

/// Spawn the worker thread. `ctx` is only used to wake the UI when a result lands.
pub fn spawn(ctx: egui::Context) -> WorkerHandle {
    let (cmd_tx, cmd_rx) = channel::<WorkerCmd>();
    let (evt_tx, evt_rx) = channel::<WorkerEvt>();
    thread::Builder::new()
        .name("rag-mcp-ui-worker".into())
        .spawn(move || {
            while let Ok(cmd) = cmd_rx.recv() {
                // A panicking job must not kill the worker (the UI would spin
                // on a pending slot forever): answer with the matching error
                // event carrying the same seq, so the slot clears.
                let fallback = panic_fallback(&cmd);
                let evt = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(cmd)))
                {
                    Ok(evt) => evt,
                    Err(_) => fallback,
                };
                // Event channel closed = window is going away; just exit.
                if evt_tx.send(evt).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        })
        .expect("spawn rag-mcp-ui worker thread");
    WorkerHandle {
        tx: cmd_tx,
        rx: evt_rx,
    }
}

fn run(cmd: WorkerCmd) -> WorkerEvt {
    match cmd {
        WorkerCmd::LoadGraph {
            seq,
            source,
            seed,
            depth,
        } => WorkerEvt::GraphLoaded {
            seq,
            result: load_cli_source(&source, seed.as_deref(), depth),
        },
        WorkerCmd::LoadWikiCatalog { seq, source } => WorkerEvt::WikiCatalog {
            seq,
            result: match &source {
                LoadSource::Http(base) => fetch_wiki_list_http(base),
                LoadSource::Db(path) => fetch_wiki_list_db(path),
            },
        },
        WorkerCmd::OpenPage {
            seq,
            pane_b,
            push_history,
            meta,
            q,
            source,
        } => {
            let result = fetch_page(meta.as_ref(), q.as_deref(), &source).map(|body| {
                let backlinks = fetch_backlinks(&body.id, &source);
                (body, backlinks)
            });
            WorkerEvt::PageOpened {
                seq,
                pane_b,
                push_history,
                q,
                result,
            }
        }
        WorkerCmd::LoadBacklinks {
            seq,
            document_id,
            source,
        } => WorkerEvt::Backlinks {
            seq,
            result: Ok(fetch_backlinks(&document_id, &source)),
            document_id,
        },
        WorkerCmd::ReadContent {
            seq,
            node_id,
            doc_id,
            uri,
            label,
            source,
        } => {
            let result = match &source {
                LoadSource::Http(base) => {
                    fetch_document_http(base, doc_id.as_deref(), uri.as_deref(), Some(&label))
                }
                LoadSource::Db(path) => fetch_document_db(path, doc_id.as_deref(), uri.as_deref()),
            };
            WorkerEvt::Content {
                seq,
                node_id,
                result,
            }
        }
        WorkerCmd::ExpandNeighbors {
            seq,
            selected,
            current,
            full,
            db_path,
            max_nodes,
        } => {
            let result = match db_path {
                Some(path) => {
                    expand_neighbors_store(&path, &current, &selected, max_nodes, full.as_ref())
                }
                None => match full {
                    Some(full) => Ok(expand_neighbors_local(
                        &full,
                        &current,
                        &selected,
                        max_nodes as usize,
                    )),
                    None => Err("Expand requires a loaded graph".into()),
                },
            };
            WorkerEvt::Expanded {
                seq,
                selected,
                result,
            }
        }
        WorkerCmd::SavePage { seq, req, source } => {
            let result = match &source {
                LoadSource::Http(base) => put_wiki_http(base, &req),
                LoadSource::Db(path) => {
                    save_wiki_db(path, &req.id, &req.title, &req.content, req.if_match_revision)
                }
            };
            WorkerEvt::SavedPage { seq, result }
        }
    }
}

/// Error event matching `cmd`, sent when the job panicked. Keeps the seq (and
/// pane routing fields) so the app clears the right pending slot.
fn panic_fallback(cmd: &WorkerCmd) -> WorkerEvt {
    fn err<T>() -> Result<T, String> {
        Err("worker job panicked (see stderr)".to_string())
    }
    match cmd {
        WorkerCmd::LoadGraph { seq, .. } => WorkerEvt::GraphLoaded {
            seq: *seq,
            result: err(),
        },
        WorkerCmd::LoadWikiCatalog { seq, .. } => WorkerEvt::WikiCatalog {
            seq: *seq,
            result: err(),
        },
        WorkerCmd::OpenPage {
            seq,
            pane_b,
            push_history,
            q,
            ..
        } => WorkerEvt::PageOpened {
            seq: *seq,
            pane_b: *pane_b,
            push_history: *push_history,
            q: q.clone(),
            result: err(),
        },
        WorkerCmd::LoadBacklinks {
            seq, document_id, ..
        } => WorkerEvt::Backlinks {
            seq: *seq,
            document_id: document_id.clone(),
            result: err(),
        },
        WorkerCmd::ReadContent { seq, node_id, .. } => WorkerEvt::Content {
            seq: *seq,
            node_id: node_id.clone(),
            result: err(),
        },
        WorkerCmd::ExpandNeighbors { seq, selected, .. } => WorkerEvt::Expanded {
            seq: *seq,
            selected: selected.clone(),
            result: err(),
        },
        WorkerCmd::SavePage { seq, .. } => WorkerEvt::SavedPage {
            seq: *seq,
            result: err(),
        },
    }
}

/// Fetch a wiki page body by catalog meta, or by exact wiki uri for the
/// unresolved-`[[link]]` fallback (no fuzzy label pick — avoids wrong page).
fn fetch_page(
    meta: Option<&WikiPageMeta>,
    q: Option<&str>,
    source: &LoadSource,
) -> Result<DocumentBody, String> {
    match (meta, source) {
        (Some(m), LoadSource::Http(base)) => {
            fetch_document_http(base, Some(&m.id), Some(&m.uri), Some(&m.title))
        }
        (Some(m), LoadSource::Db(path)) => fetch_document_db(path, Some(&m.id), Some(&m.uri)),
        (None, LoadSource::Http(base)) => {
            let q = q.unwrap_or_default();
            fetch_document_http(base, None, Some(&format!("wiki://{q}")), None)
                .or_else(|_| fetch_document_http(base, None, Some(q), None))
        }
        (None, LoadSource::Db(path)) => {
            let q = q.unwrap_or_default();
            fetch_document_db(path, None, Some(&format!("wiki://{q}")))
                .or_else(|_| fetch_document_db(path, None, Some(q)))
        }
    }
}

/// Backlinks for a document id; transport errors degrade to an empty list
/// (same policy as the previous synchronous path).
fn fetch_backlinks(document_id: &str, source: &LoadSource) -> Vec<BacklinkItem> {
    match source {
        LoadSource::Http(base) => fetch_backlinks_http(base, document_id).unwrap_or_default(),
        LoadSource::Db(path) => {
            if let Ok(store) = rag_mcp::Store::open(path) {
                if let Ok(rows) = store.wiki_backlinks_for_document(document_id) {
                    return rows
                        .into_iter()
                        .map(|(label, id)| BacklinkItem { label, id })
                        .collect();
                }
            }
            Vec::new()
        }
    }
}
