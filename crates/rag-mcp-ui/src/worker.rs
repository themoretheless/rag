//! Background worker thread: all blocking IO (HTTP / DuckDB) off the UI thread.
//!
//! EGUI_GRAPH_VIEW §2.5 / §8.3: the egui frame thread must never touch DuckDB or
//! blocking network. `GraphApp` sends [`WorkerCmd`] values carrying everything the
//! job needs (sources are cheap clones; views are ≤ UI hard cap), the worker runs
//! them serially and answers with [`WorkerEvt`]. Each job carries a `seq`; the app
//! ignores events whose `seq` no longer matches the pending slot (stale answers
//! after a newer request was issued).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use rag_mcp::GraphView;

use crate::load::{
    expand_neighbors_http, expand_neighbors_local, expand_neighbors_store, fetch_activity_http,
    fetch_backlinks_http, fetch_document_db, fetch_document_http, fetch_wiki_list_db,
    fetch_wiki_list_http, load_cli_source, put_wiki_http, save_wiki_db, ActivityEvent,
    BacklinkItem, CliSource, DocumentBody, GraphSourceKind, LoadedGraph, WikiPageMeta,
    WikiPutRequest,
};
use crate::operations::{
    backup_http, cancel_job_http, checkpoint_http, fetch_jobs_http, fetch_operations_http,
    start_sync_job_http, BackupRequest, JobSnapshot, MaintenanceResult, OperationsSnapshot,
    SyncJobRequest,
};
use crate::product::{
    fetch_library_http, fetch_project_home_http, LibraryPage, LibraryRequest, ProjectHome,
};
use crate::revisions::{
    fetch_revision_diff_http, fetch_revision_snapshot_http, fetch_revisions_http,
    restore_revision_http, RestoreRevisionResult, RevisionDiff, RevisionPage,
};
use crate::search::{fetch_search_http, SearchRequest, SearchResults};

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
        project: Option<String>,
        include_tags: bool,
    },
    /// Project-scoped inventory for the Project Home dashboard.
    LoadProjectHome {
        seq: u64,
        base: String,
        project: String,
    },
    /// One server-filtered page of the lean Unified Library catalog.
    LoadLibrary {
        seq: u64,
        base: String,
        request: LibraryRequest,
    },
    /// Full body for the selected Unified Library row.
    LoadLibraryDocument {
        seq: u64,
        base: String,
        document_id: String,
        uri: String,
    },
    /// Search over chunks with the gateway's retrieval stack.
    Search {
        seq: u64,
        base: String,
        request: SearchRequest,
    },
    LoadOperations {
        seq: u64,
        base: String,
    },
    LoadJobs {
        seq: u64,
        base: String,
    },
    StartSyncJob {
        seq: u64,
        base: String,
        request: SyncJobRequest,
    },
    CancelJob {
        seq: u64,
        base: String,
        id: String,
    },
    Checkpoint {
        seq: u64,
        base: String,
    },
    Backup {
        seq: u64,
        base: String,
        request: BackupRequest,
    },
    LoadRevisions {
        seq: u64,
        base: String,
        document_id: String,
        cursor: Option<String>,
        append: bool,
    },
    LoadRevisionSnapshot {
        seq: u64,
        base: String,
        document_id: String,
        revision: i64,
    },
    LoadRevisionDiff {
        seq: u64,
        base: String,
        document_id: String,
        from_revision: i64,
        to_revision: Option<i64>,
    },
    RestoreRevision {
        seq: u64,
        base: String,
        document_id: String,
        revision: i64,
        if_match_revision: i64,
    },
    /// Wiki catalog list for the sidebar.
    LoadWikiCatalog {
        seq: u64,
        source: LoadSource,
        project: Option<String>,
    },
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
        http_base: Option<String>,
        project: Option<String>,
        include_tags: bool,
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
    LoadActivity {
        seq: u64,
        base: String,
    },
}

/// Results delivered back to the UI thread.
#[derive(Debug)]
pub enum WorkerEvt {
    GraphLoaded {
        seq: u64,
        result: Result<LoadedGraph, String>,
    },
    ProjectHomeLoaded {
        seq: u64,
        project: String,
        result: Result<ProjectHome, String>,
    },
    LibraryLoaded {
        seq: u64,
        project: Option<String>,
        result: Result<LibraryPage, String>,
    },
    LibraryDocumentLoaded {
        seq: u64,
        document_id: String,
        result: Result<DocumentBody, String>,
    },
    SearchLoaded {
        seq: u64,
        project: Option<String>,
        result: Result<SearchResults, String>,
    },
    OperationsLoaded {
        seq: u64,
        result: Result<OperationsSnapshot, String>,
    },
    JobsLoaded {
        seq: u64,
        result: Result<Vec<JobSnapshot>, String>,
    },
    JobChanged {
        seq: u64,
        result: Box<Result<JobSnapshot, String>>,
    },
    MaintenanceCompleted {
        seq: u64,
        result: Result<MaintenanceResult, String>,
    },
    RevisionsLoaded {
        seq: u64,
        document_id: String,
        append: bool,
        result: Result<RevisionPage, String>,
    },
    RevisionSnapshotLoaded {
        seq: u64,
        document_id: String,
        revision: i64,
        result: Result<DocumentBody, String>,
    },
    RevisionDiffLoaded {
        seq: u64,
        document_id: String,
        from_revision: i64,
        result: Result<RevisionDiff, String>,
    },
    RevisionRestored {
        seq: u64,
        document_id: String,
        result: Result<RestoreRevisionResult, String>,
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
    Activity {
        seq: u64,
        result: Result<Vec<ActivityEvent>, String>,
    },
}

/// Replaceable read lanes. A newly submitted command supersedes queued work in
/// the same lane; mutation commands deliberately have no slot and always run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WorkerSlot {
    Graph,
    ProjectHome,
    Library,
    LibraryDocument,
    Search,
    Operations,
    Jobs,
    Revisions,
    RevisionSnapshot,
    RevisionDiff,
    WikiCatalog,
    PageA,
    PageB,
    Backlinks,
    Content,
    Expand,
    Activity,
}

impl WorkerCmd {
    fn slot_and_seq(&self) -> Option<(WorkerSlot, u64)> {
        let (slot, seq) = match self {
            Self::LoadGraph { seq, .. } => (WorkerSlot::Graph, *seq),
            Self::LoadProjectHome { seq, .. } => (WorkerSlot::ProjectHome, *seq),
            Self::LoadLibrary { seq, .. } => (WorkerSlot::Library, *seq),
            Self::LoadLibraryDocument { seq, .. } => (WorkerSlot::LibraryDocument, *seq),
            Self::Search { seq, .. } => (WorkerSlot::Search, *seq),
            Self::LoadOperations { seq, .. } => (WorkerSlot::Operations, *seq),
            Self::LoadJobs { seq, .. } => (WorkerSlot::Jobs, *seq),
            Self::LoadRevisions { seq, .. } => (WorkerSlot::Revisions, *seq),
            Self::LoadRevisionSnapshot { seq, .. } => (WorkerSlot::RevisionSnapshot, *seq),
            Self::LoadRevisionDiff { seq, .. } => (WorkerSlot::RevisionDiff, *seq),
            Self::LoadWikiCatalog { seq, .. } => (WorkerSlot::WikiCatalog, *seq),
            Self::OpenPage {
                seq, pane_b: true, ..
            } => (WorkerSlot::PageB, *seq),
            Self::OpenPage {
                seq, pane_b: false, ..
            } => (WorkerSlot::PageA, *seq),
            Self::LoadBacklinks { seq, .. } => (WorkerSlot::Backlinks, *seq),
            Self::ReadContent { seq, .. } => (WorkerSlot::Content, *seq),
            Self::ExpandNeighbors { seq, .. } => (WorkerSlot::Expand, *seq),
            Self::LoadActivity { seq, .. } => (WorkerSlot::Activity, *seq),
            Self::StartSyncJob { .. }
            | Self::CancelJob { .. }
            | Self::Checkpoint { .. }
            | Self::Backup { .. }
            | Self::RestoreRevision { .. }
            | Self::SavePage { .. } => return None,
        };
        Some((slot, seq))
    }

    /// Lower values run first after the worker drains its input queue. Product
    /// workspaces must not sit behind speculative or stale Wiki reads.
    fn priority(&self) -> u8 {
        match self {
            Self::StartSyncJob { .. }
            | Self::CancelJob { .. }
            | Self::Checkpoint { .. }
            | Self::Backup { .. }
            | Self::RestoreRevision { .. }
            | Self::SavePage { .. } => 0,
            Self::LoadProjectHome { .. } | Self::LoadLibrary { .. } | Self::Search { .. } => 1,
            Self::LoadGraph { .. }
            | Self::LoadLibraryDocument { .. }
            | Self::LoadOperations { .. }
            | Self::LoadJobs { .. }
            | Self::LoadRevisions { .. }
            | Self::LoadRevisionSnapshot { .. }
            | Self::LoadRevisionDiff { .. }
            | Self::ReadContent { .. }
            | Self::ExpandNeighbors { .. }
            | Self::LoadActivity { .. } => 2,
            Self::LoadWikiCatalog { .. } | Self::OpenPage { .. } | Self::LoadBacklinks { .. } => 3,
        }
    }
}

/// Channel endpoints owned by `GraphApp`.
pub struct WorkerHandle {
    tx: Sender<WorkerCmd>,
    pub rx: Receiver<WorkerEvt>,
    latest: Arc<Mutex<HashMap<WorkerSlot, u64>>>,
}

impl WorkerHandle {
    pub fn send(&self, cmd: WorkerCmd) {
        if let Some((slot, seq)) = cmd.slot_and_seq() {
            lock_latest(&self.latest).insert(slot, seq);
        }
        // A dead worker means the window is going away; ignore send errors.
        let _ = self.tx.send(cmd);
    }

    /// Invalidate reads whose answer belongs to the previous project. The app
    /// clears its matching pending slots at the same transition.
    pub fn cancel_project_scoped_reads(&self) {
        self.cancel_slots(&[
            WorkerSlot::Graph,
            WorkerSlot::ProjectHome,
            WorkerSlot::Library,
            WorkerSlot::LibraryDocument,
            WorkerSlot::Search,
            WorkerSlot::Revisions,
            WorkerSlot::RevisionSnapshot,
            WorkerSlot::RevisionDiff,
            WorkerSlot::WikiCatalog,
            WorkerSlot::PageA,
            WorkerSlot::PageB,
            WorkerSlot::Backlinks,
            WorkerSlot::Content,
            WorkerSlot::Expand,
        ]);
    }

    /// Invalidate only Wiki reads when its project catalog is reset.
    pub fn cancel_wiki_reads(&self) {
        self.cancel_slots(&[
            WorkerSlot::WikiCatalog,
            WorkerSlot::PageA,
            WorkerSlot::PageB,
            WorkerSlot::Backlinks,
        ]);
    }

    /// Invalidate topology-dependent work before a focus/tag/source reload.
    pub fn cancel_topology_reads(&self) {
        self.cancel_slots(&[WorkerSlot::Graph, WorkerSlot::Content, WorkerSlot::Expand]);
    }

    /// Invalidate an expansion before rebuilding the in-memory neighborhood.
    pub fn cancel_expand_read(&self) {
        self.cancel_slots(&[WorkerSlot::Expand]);
    }

    fn cancel_slots(&self, slots: &[WorkerSlot]) {
        let mut latest = lock_latest(&self.latest);
        for slot in slots {
            latest.insert(*slot, u64::MAX);
        }
    }
}

fn lock_latest(
    latest: &Mutex<HashMap<WorkerSlot, u64>>,
) -> std::sync::MutexGuard<'_, HashMap<WorkerSlot, u64>> {
    latest
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn command_is_current(cmd: &WorkerCmd, latest: &Mutex<HashMap<WorkerSlot, u64>>) -> bool {
    cmd.slot_and_seq()
        .is_none_or(|(slot, seq)| lock_latest(latest).get(&slot).copied() == Some(seq))
}

/// Spawn the worker thread. `ctx` is only used to wake the UI when a result lands.
pub fn spawn(ctx: egui::Context) -> WorkerHandle {
    let (cmd_tx, cmd_rx) = channel::<WorkerCmd>();
    let (evt_tx, evt_rx) = channel::<WorkerEvt>();
    let latest = Arc::new(Mutex::new(HashMap::new()));
    let worker_latest = Arc::clone(&latest);
    thread::Builder::new()
        .name("rag-mcp-ui-worker".into())
        .spawn(move || {
            let mut queued = VecDeque::new();
            loop {
                if queued.is_empty() {
                    let Ok(cmd) = cmd_rx.recv() else {
                        break;
                    };
                    queued.push_back(cmd);
                }
                queued.extend(cmd_rx.try_iter());
                queued.retain(|cmd| command_is_current(cmd, &worker_latest));
                let Some(next) = queued
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, cmd)| cmd.priority())
                    .map(|(index, _)| index)
                else {
                    continue;
                };
                let cmd = queued
                    .remove(next)
                    .expect("priority index comes from the worker queue");
                if !command_is_current(&cmd, &worker_latest) {
                    continue;
                }
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
        latest,
    }
}

fn run(cmd: WorkerCmd) -> WorkerEvt {
    match cmd {
        WorkerCmd::LoadGraph {
            seq,
            source,
            seed,
            depth,
            project,
            include_tags,
        } => WorkerEvt::GraphLoaded {
            seq,
            result: load_cli_source(
                &source,
                seed.as_deref(),
                depth,
                project.as_deref(),
                include_tags,
            ),
        },
        WorkerCmd::LoadProjectHome { seq, base, project } => WorkerEvt::ProjectHomeLoaded {
            seq,
            result: fetch_project_home_http(&base, &project),
            project,
        },
        WorkerCmd::LoadLibrary { seq, base, request } => {
            let project = clean_project(&request.wing);
            WorkerEvt::LibraryLoaded {
                seq,
                project,
                result: fetch_library_http(&base, &request),
            }
        }
        WorkerCmd::LoadLibraryDocument {
            seq,
            base,
            document_id,
            uri,
        } => WorkerEvt::LibraryDocumentLoaded {
            seq,
            result: fetch_document_http(&base, Some(&document_id), Some(&uri), None),
            document_id,
        },
        WorkerCmd::Search { seq, base, request } => {
            let project = request.wing.as_deref().and_then(clean_project);
            WorkerEvt::SearchLoaded {
                seq,
                project,
                result: fetch_search_http(&base, &request),
            }
        }
        WorkerCmd::LoadOperations { seq, base } => WorkerEvt::OperationsLoaded {
            seq,
            result: fetch_operations_http(&base),
        },
        WorkerCmd::LoadJobs { seq, base } => WorkerEvt::JobsLoaded {
            seq,
            result: fetch_jobs_http(&base),
        },
        WorkerCmd::StartSyncJob { seq, base, request } => WorkerEvt::JobChanged {
            seq,
            result: Box::new(start_sync_job_http(&base, &request)),
        },
        WorkerCmd::CancelJob { seq, base, id } => WorkerEvt::JobChanged {
            seq,
            result: Box::new(cancel_job_http(&base, &id)),
        },
        WorkerCmd::Checkpoint { seq, base } => WorkerEvt::MaintenanceCompleted {
            seq,
            result: checkpoint_http(&base),
        },
        WorkerCmd::Backup { seq, base, request } => WorkerEvt::MaintenanceCompleted {
            seq,
            result: backup_http(&base, &request),
        },
        WorkerCmd::LoadRevisions {
            seq,
            base,
            document_id,
            cursor,
            append,
        } => WorkerEvt::RevisionsLoaded {
            seq,
            result: fetch_revisions_http(&base, &document_id, cursor.as_deref()),
            document_id,
            append,
        },
        WorkerCmd::LoadRevisionSnapshot {
            seq,
            base,
            document_id,
            revision,
        } => WorkerEvt::RevisionSnapshotLoaded {
            seq,
            result: fetch_revision_snapshot_http(&base, &document_id, revision),
            document_id,
            revision,
        },
        WorkerCmd::LoadRevisionDiff {
            seq,
            base,
            document_id,
            from_revision,
            to_revision,
        } => WorkerEvt::RevisionDiffLoaded {
            seq,
            result: fetch_revision_diff_http(&base, &document_id, from_revision, to_revision),
            document_id,
            from_revision,
        },
        WorkerCmd::RestoreRevision {
            seq,
            base,
            document_id,
            revision,
            if_match_revision,
        } => WorkerEvt::RevisionRestored {
            seq,
            result: restore_revision_http(&base, &document_id, revision, if_match_revision),
            document_id,
        },
        WorkerCmd::LoadWikiCatalog {
            seq,
            source,
            project,
        } => WorkerEvt::WikiCatalog {
            seq,
            result: match &source {
                LoadSource::Http(base) => fetch_wiki_list_http(base, project.as_deref()),
                LoadSource::Db(path) => fetch_wiki_list_db(path, project.as_deref()),
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
            http_base,
            project,
            include_tags,
            max_nodes,
        } => {
            let result = match (http_base, db_path) {
                (Some(base), _) => expand_neighbors_http(
                    &base,
                    &current,
                    &selected,
                    max_nodes,
                    project.as_deref(),
                    include_tags,
                ),
                (None, Some(path)) => {
                    expand_neighbors_store(&path, &current, &selected, max_nodes, full.as_ref())
                }
                (None, None) => match full {
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
                LoadSource::Db(path) => save_wiki_db(
                    path,
                    &req.id,
                    &req.title,
                    &req.content,
                    req.if_match_revision,
                ),
            };
            WorkerEvt::SavedPage { seq, result }
        }
        WorkerCmd::LoadActivity { seq, base } => WorkerEvt::Activity {
            seq,
            result: fetch_activity_http(&base),
        },
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
        WorkerCmd::LoadProjectHome { seq, project, .. } => WorkerEvt::ProjectHomeLoaded {
            seq: *seq,
            project: project.clone(),
            result: err(),
        },
        WorkerCmd::LoadLibrary { seq, request, .. } => WorkerEvt::LibraryLoaded {
            seq: *seq,
            project: clean_project(&request.wing),
            result: err(),
        },
        WorkerCmd::LoadLibraryDocument {
            seq, document_id, ..
        } => WorkerEvt::LibraryDocumentLoaded {
            seq: *seq,
            document_id: document_id.clone(),
            result: err(),
        },
        WorkerCmd::Search { seq, request, .. } => WorkerEvt::SearchLoaded {
            seq: *seq,
            project: request.wing.as_deref().and_then(clean_project),
            result: err(),
        },
        WorkerCmd::LoadOperations { seq, .. } => WorkerEvt::OperationsLoaded {
            seq: *seq,
            result: err(),
        },
        WorkerCmd::LoadJobs { seq, .. } => WorkerEvt::JobsLoaded {
            seq: *seq,
            result: err(),
        },
        WorkerCmd::StartSyncJob { seq, .. } | WorkerCmd::CancelJob { seq, .. } => {
            WorkerEvt::JobChanged {
                seq: *seq,
                result: Box::new(err()),
            }
        }
        WorkerCmd::Checkpoint { seq, .. } | WorkerCmd::Backup { seq, .. } => {
            WorkerEvt::MaintenanceCompleted {
                seq: *seq,
                result: err(),
            }
        }
        WorkerCmd::LoadRevisions {
            seq,
            document_id,
            append,
            ..
        } => WorkerEvt::RevisionsLoaded {
            seq: *seq,
            document_id: document_id.clone(),
            append: *append,
            result: err(),
        },
        WorkerCmd::LoadRevisionSnapshot {
            seq,
            document_id,
            revision,
            ..
        } => WorkerEvt::RevisionSnapshotLoaded {
            seq: *seq,
            document_id: document_id.clone(),
            revision: *revision,
            result: err(),
        },
        WorkerCmd::LoadRevisionDiff {
            seq,
            document_id,
            from_revision,
            ..
        } => WorkerEvt::RevisionDiffLoaded {
            seq: *seq,
            document_id: document_id.clone(),
            from_revision: *from_revision,
            result: err(),
        },
        WorkerCmd::RestoreRevision {
            seq, document_id, ..
        } => WorkerEvt::RevisionRestored {
            seq: *seq,
            document_id: document_id.clone(),
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
        WorkerCmd::LoadActivity { seq, .. } => WorkerEvt::Activity {
            seq: *seq,
            result: err(),
        },
    }
}

fn clean_project(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn library(seq: u64, wing: &str) -> WorkerCmd {
        WorkerCmd::LoadLibrary {
            seq,
            base: "http://gateway".into(),
            request: LibraryRequest {
                wing: wing.into(),
                ..LibraryRequest::default()
            },
        }
    }

    #[test]
    fn newer_read_supersedes_queued_work_in_the_same_lane() {
        let latest = Mutex::new(HashMap::new());
        let old = library(1, "alpha");
        let new = library(2, "beta");
        let (slot, old_seq) = old.slot_and_seq().expect("replaceable read");
        lock_latest(&latest).insert(slot, old_seq);
        assert!(command_is_current(&old, &latest));
        let (_, new_seq) = new.slot_and_seq().expect("replaceable read");
        lock_latest(&latest).insert(slot, new_seq);
        assert!(!command_is_current(&old, &latest));
        assert!(command_is_current(&new, &latest));
    }

    #[test]
    fn product_reads_are_prioritized_over_wiki_catalog_warmup() {
        let wiki = WorkerCmd::LoadWikiCatalog {
            seq: 1,
            source: LoadSource::Http("http://gateway".into()),
            project: Some("alpha".into()),
        };
        assert!(library(2, "alpha").priority() < wiki.priority());
    }

    #[test]
    fn project_scope_is_copied_into_library_and_search_events() {
        let library = panic_fallback(&library(1, " alpha "));
        assert!(matches!(
            library,
            WorkerEvt::LibraryLoaded {
                project: Some(project),
                ..
            } if project == "alpha"
        ));
        let search = panic_fallback(&WorkerCmd::Search {
            seq: 2,
            base: "http://gateway".into(),
            request: SearchRequest {
                wing: Some(" beta ".into()),
                ..SearchRequest::default()
            },
        });
        assert!(matches!(
            search,
            WorkerEvt::SearchLoaded {
                project: Some(project),
                ..
            } if project == "beta"
        ));
    }
}
