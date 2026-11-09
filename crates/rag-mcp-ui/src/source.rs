//! Where topology, documents and wiki pages come from.
//!
//! [`DocumentSource`] is the single seam between the background worker and a
//! backend: the HTTP gateway, a read-only DuckDB file (`direct-db` feature) or
//! a `graph.json` snapshot. Worker commands carry an `Arc<dyn DocumentSource>`,
//! so the worker never matches on the backend kind.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rag_mcp_core::GraphView;

use crate::cli::CliSource;
use crate::dto::{
    BacklinkItem, DocumentBody, GraphSourceKind, LoadedGraph, WikiPageMeta, WikiPutRequest,
};
use crate::http::{
    expand_neighbors_http, fetch_backlinks_http, fetch_document_http, fetch_wiki_list_http,
    load_http, put_wiki_http,
};
use crate::snapshot::load_snapshot_path;
use crate::topology::expand_neighbors_local;

/// One-hop expansion of `selected`, merged into `current` under `max_nodes`.
#[derive(Debug, Clone, Copy)]
pub struct ExpandRequest<'a> {
    pub current: &'a GraphView,
    /// Whole loaded topology; only sources that expand in memory read it.
    pub full: Option<&'a GraphView>,
    pub selected: &'a str,
    pub max_nodes: u32,
    pub project: Option<&'a str>,
    pub include_tags: bool,
}

/// Backend for graph topology, document bodies and wiki pages.
///
/// Every method blocks on IO and is called from the worker thread only.
pub trait DocumentSource: fmt::Debug + Send + Sync {
    /// Initial / retry topology load.
    fn load_graph(
        &self,
        seed: Option<&str>,
        depth: u32,
        project: Option<&str>,
        include_tags: bool,
    ) -> Result<LoadedGraph, String>;

    /// Wiki catalog for the browser list.
    fn wiki_catalog(&self, project: Option<&str>) -> Result<Vec<WikiPageMeta>, String>;

    /// One document body by id and/or uri. `label` only helps the HTTP resolver.
    fn document(
        &self,
        doc_id: Option<&str>,
        uri: Option<&str>,
        label: Option<&str>,
    ) -> Result<DocumentBody, String>;

    /// Backlinks for a document id. Transport/store errors remain explicit so the
    /// UI cannot misreport an unavailable query as "No incoming links".
    fn backlinks(
        &self,
        document_id: &str,
        project: Option<&str>,
    ) -> Result<Vec<BacklinkItem>, String>;

    /// Save a wiki page. Only the HTTP gateway accepts writes.
    fn save_wiki(&self, req: &WikiPutRequest) -> Result<DocumentBody, String>;

    /// One-hop neighbour expansion of the selected node.
    fn expand(&self, request: &ExpandRequest<'_>) -> Result<GraphView, String>;

    /// True when [`DocumentSource::save_wiki`] can succeed.
    fn is_writable(&self) -> bool {
        false
    }

    /// True when [`DocumentSource::expand`] needs [`ExpandRequest::full`].
    fn expands_in_memory(&self) -> bool {
        false
    }
}

/// Writable rag-mcp HTTP gateway (the process that owns DuckDB).
#[derive(Debug, Clone)]
pub struct HttpSource {
    base: String,
}

impl HttpSource {
    /// Bind to a gateway base URL such as `http://127.0.0.1:7432`.
    pub fn new(base: impl Into<String>) -> Self {
        Self { base: base.into() }
    }
}

impl DocumentSource for HttpSource {
    fn load_graph(
        &self,
        seed: Option<&str>,
        depth: u32,
        project: Option<&str>,
        include_tags: bool,
    ) -> Result<LoadedGraph, String> {
        load_http(&self.base, seed, depth, project, include_tags)
    }

    fn wiki_catalog(&self, project: Option<&str>) -> Result<Vec<WikiPageMeta>, String> {
        fetch_wiki_list_http(&self.base, project)
    }

    fn document(
        &self,
        doc_id: Option<&str>,
        uri: Option<&str>,
        label: Option<&str>,
    ) -> Result<DocumentBody, String> {
        fetch_document_http(&self.base, doc_id, uri, label)
    }

    fn backlinks(
        &self,
        document_id: &str,
        project: Option<&str>,
    ) -> Result<Vec<BacklinkItem>, String> {
        fetch_backlinks_http(&self.base, document_id, project)
    }

    fn save_wiki(&self, req: &WikiPutRequest) -> Result<DocumentBody, String> {
        put_wiki_http(&self.base, req)
    }

    fn expand(&self, request: &ExpandRequest<'_>) -> Result<GraphView, String> {
        expand_neighbors_http(
            &self.base,
            request.current,
            request.selected,
            request.max_nodes,
            request.project,
            request.include_tags,
        )
    }

    fn is_writable(&self) -> bool {
        true
    }
}

const SNAPSHOT_HAS_NO_DOCUMENTS: &str =
    "snapshot mode has topology only; use --http or --db for documents and wiki pages";
pub(crate) const READ_ONLY_SOURCE: &str =
    "direct --db mode is read-only; use the HTTP gateway to rebuild derived state";

/// `graph.json` snapshot (Mode C): topology only, expanded in memory.
#[derive(Debug, Clone)]
pub struct SnapshotSource {
    path: PathBuf,
}

impl SnapshotSource {
    /// Bind to a snapshot file path (read on every graph load).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl DocumentSource for SnapshotSource {
    fn load_graph(
        &self,
        _seed: Option<&str>,
        _depth: u32,
        _project: Option<&str>,
        _include_tags: bool,
    ) -> Result<LoadedGraph, String> {
        load_snapshot_path(&self.path)
    }

    fn wiki_catalog(&self, _project: Option<&str>) -> Result<Vec<WikiPageMeta>, String> {
        Err(SNAPSHOT_HAS_NO_DOCUMENTS.into())
    }

    fn document(
        &self,
        _doc_id: Option<&str>,
        _uri: Option<&str>,
        _label: Option<&str>,
    ) -> Result<DocumentBody, String> {
        Err(SNAPSHOT_HAS_NO_DOCUMENTS.into())
    }

    fn backlinks(
        &self,
        _document_id: &str,
        _project: Option<&str>,
    ) -> Result<Vec<BacklinkItem>, String> {
        Err(SNAPSHOT_HAS_NO_DOCUMENTS.into())
    }

    fn save_wiki(&self, _req: &WikiPutRequest) -> Result<DocumentBody, String> {
        Err(SNAPSHOT_HAS_NO_DOCUMENTS.into())
    }

    fn expand(&self, request: &ExpandRequest<'_>) -> Result<GraphView, String> {
        expand_in_memory(request)
    }

    fn expands_in_memory(&self) -> bool {
        true
    }
}

fn expand_in_memory(request: &ExpandRequest<'_>) -> Result<GraphView, String> {
    match request.full {
        Some(full) => Ok(expand_neighbors_local(
            full,
            request.current,
            request.selected,
            request.max_nodes as usize,
        )),
        None => Err("Expand requires a loaded graph".into()),
    }
}

const NO_BACKEND: &str = "no data source is connected";

/// Already-loaded topology without a backend: expansion only.
#[derive(Debug, Clone, Copy)]
struct InMemorySource;

/// Source that expands over the already-loaded full view and serves nothing else.
pub fn in_memory() -> Arc<dyn DocumentSource> {
    Arc::new(InMemorySource)
}

impl DocumentSource for InMemorySource {
    fn load_graph(
        &self,
        _seed: Option<&str>,
        _depth: u32,
        _project: Option<&str>,
        _include_tags: bool,
    ) -> Result<LoadedGraph, String> {
        Err(NO_BACKEND.into())
    }

    fn wiki_catalog(&self, _project: Option<&str>) -> Result<Vec<WikiPageMeta>, String> {
        Err(NO_BACKEND.into())
    }

    fn document(
        &self,
        _doc_id: Option<&str>,
        _uri: Option<&str>,
        _label: Option<&str>,
    ) -> Result<DocumentBody, String> {
        Err(NO_BACKEND.into())
    }

    fn backlinks(
        &self,
        _document_id: &str,
        _project: Option<&str>,
    ) -> Result<Vec<BacklinkItem>, String> {
        Err(NO_BACKEND.into())
    }

    fn save_wiki(&self, _req: &WikiPutRequest) -> Result<DocumentBody, String> {
        Err(NO_BACKEND.into())
    }

    fn expand(&self, request: &ExpandRequest<'_>) -> Result<GraphView, String> {
        expand_in_memory(request)
    }

    fn expands_in_memory(&self) -> bool {
        true
    }
}

#[cfg(not(feature = "direct-db"))]
pub(crate) const DIRECT_DB_DISABLED: &str =
    "rag-mcp-ui was built without the `direct-db` feature; use --http with the gateway";

/// `--db` in a build without the `direct-db` feature: every call reports that
/// the feature is missing instead of opening DuckDB.
#[cfg(not(feature = "direct-db"))]
#[derive(Debug, Clone, Copy)]
struct DisabledDbSource;

#[cfg(not(feature = "direct-db"))]
impl DocumentSource for DisabledDbSource {
    fn load_graph(
        &self,
        _seed: Option<&str>,
        _depth: u32,
        _project: Option<&str>,
        _include_tags: bool,
    ) -> Result<LoadedGraph, String> {
        Err(DIRECT_DB_DISABLED.into())
    }

    fn wiki_catalog(&self, _project: Option<&str>) -> Result<Vec<WikiPageMeta>, String> {
        Err(DIRECT_DB_DISABLED.into())
    }

    fn document(
        &self,
        _doc_id: Option<&str>,
        _uri: Option<&str>,
        _label: Option<&str>,
    ) -> Result<DocumentBody, String> {
        Err(DIRECT_DB_DISABLED.into())
    }

    fn backlinks(
        &self,
        _document_id: &str,
        _project: Option<&str>,
    ) -> Result<Vec<BacklinkItem>, String> {
        Err(format!("backlinks unavailable: {DIRECT_DB_DISABLED}"))
    }

    fn save_wiki(&self, _req: &WikiPutRequest) -> Result<DocumentBody, String> {
        Err(READ_ONLY_SOURCE.into())
    }

    fn expand(&self, _request: &ExpandRequest<'_>) -> Result<GraphView, String> {
        Err(DIRECT_DB_DISABLED.into())
    }
}

/// Hands out sources for the app's configured / loaded backends.
///
/// HTTP and snapshot sources are stateless and built on demand. The DuckDB
/// source owns the read-only connection, so it is created once per path and
/// shared by every command that targets that file.
#[derive(Debug, Default)]
pub struct SourceRegistry {
    db: Mutex<Option<(PathBuf, Arc<dyn DocumentSource>)>>,
}

impl SourceRegistry {
    /// Source for a CLI / connect-screen selection (topology loads).
    pub fn for_cli(&self, source: &CliSource) -> Arc<dyn DocumentSource> {
        match source {
            CliSource::Snapshot(path) => Arc::new(SnapshotSource::new(path)),
            CliSource::Db(path) => self.db(path),
            CliSource::Http(base) => Arc::new(HttpSource::new(base.as_str())),
        }
    }

    /// Source able to serve documents and wiki pages for a loaded graph;
    /// snapshots carry topology only.
    pub fn for_loaded(&self, source: &GraphSourceKind) -> Option<Arc<dyn DocumentSource>> {
        match source {
            GraphSourceKind::HttpService { base } => Some(Arc::new(HttpSource::new(base.as_str()))),
            GraphSourceKind::LiveStore { path } => Some(self.db(path)),
            GraphSourceKind::SnapshotFile { .. } | GraphSourceKind::VaultGraphJson { .. } => None,
        }
    }

    fn db(&self, path: &std::path::Path) -> Arc<dyn DocumentSource> {
        let mut slot = self
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((known, source)) = slot.as_ref() {
            if known == path {
                return Arc::clone(source);
            }
        }
        let source = new_db_source(path);
        *slot = Some((path.to_path_buf(), Arc::clone(&source)));
        source
    }
}

#[cfg(feature = "direct-db")]
fn new_db_source(path: &std::path::Path) -> Arc<dyn DocumentSource> {
    Arc::new(crate::direct_db::DbSource::new(path))
}

#[cfg(not(feature = "direct-db"))]
fn new_db_source(_path: &std::path::Path) -> Arc<dyn DocumentSource> {
    Arc::new(DisabledDbSource)
}

#[cfg(test)]
pub(crate) mod testing {
    //! Scripted [`DocumentSource`] that records every call.

    use super::*;

    /// Fake backend: `document` answers from `documents` by uri/id and records
    /// each lookup key; the other methods return fixed values.
    #[derive(Debug, Default)]
    pub(crate) struct FakeSource {
        pub(crate) documents: Vec<DocumentBody>,
        pub(crate) calls: Mutex<Vec<String>>,
        pub(crate) writable: bool,
        pub(crate) in_memory: bool,
    }

    impl FakeSource {
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    pub(crate) fn body(id: &str, uri: &str) -> DocumentBody {
        DocumentBody {
            id: id.into(),
            uri: uri.into(),
            title: id.into(),
            layer: "wiki".into(),
            kind: "page".into(),
            content: String::new(),
            content_hash: None,
            updated_at: None,
            revision: Some(1),
            etag: None,
        }
    }

    impl DocumentSource for FakeSource {
        fn load_graph(
            &self,
            seed: Option<&str>,
            depth: u32,
            project: Option<&str>,
            include_tags: bool,
        ) -> Result<LoadedGraph, String> {
            self.record(format!(
                "load_graph {seed:?} {depth} {project:?} {include_tags}"
            ));
            Err("fake source has no graph".into())
        }

        fn wiki_catalog(&self, project: Option<&str>) -> Result<Vec<WikiPageMeta>, String> {
            self.record(format!("wiki_catalog {project:?}"));
            Ok(Vec::new())
        }

        fn document(
            &self,
            doc_id: Option<&str>,
            uri: Option<&str>,
            label: Option<&str>,
        ) -> Result<DocumentBody, String> {
            self.record(format!("document {doc_id:?} {uri:?} {label:?}"));
            self.documents
                .iter()
                .find(|doc| Some(doc.id.as_str()) == doc_id || Some(doc.uri.as_str()) == uri)
                .cloned()
                .ok_or_else(|| "not found".to_string())
        }

        fn backlinks(
            &self,
            document_id: &str,
            project: Option<&str>,
        ) -> Result<Vec<BacklinkItem>, String> {
            self.record(format!("backlinks {document_id} {project:?}"));
            Ok(Vec::new())
        }

        fn save_wiki(&self, req: &WikiPutRequest) -> Result<DocumentBody, String> {
            self.record(format!("save_wiki {}", req.slug));
            Ok(body(&req.id, req.uri.as_deref().unwrap_or_default()))
        }

        fn expand(&self, request: &ExpandRequest<'_>) -> Result<GraphView, String> {
            self.record(format!(
                "expand {} {} {:?} {} full={}",
                request.selected,
                request.max_nodes,
                request.project,
                request.include_tags,
                request.full.is_some()
            ));
            Ok(request.current.clone())
        }

        fn is_writable(&self) -> bool {
            self.writable
        }

        fn expands_in_memory(&self) -> bool {
            self.in_memory
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_maps_cli_and_loaded_sources_to_capabilities() {
        let registry = SourceRegistry::default();
        let http = registry.for_cli(&CliSource::Http("http://gateway".into()));
        assert!(http.is_writable());
        assert!(!http.expands_in_memory());

        let snapshot = registry.for_cli(&CliSource::Snapshot("graph.json".into()));
        assert!(!snapshot.is_writable());
        assert!(snapshot.expands_in_memory());

        let loaded_snapshot = GraphSourceKind::SnapshotFile {
            path: "graph.json".into(),
            mtime: None,
        };
        assert!(registry.for_loaded(&loaded_snapshot).is_none());
        let loaded_http = GraphSourceKind::HttpService {
            base: "http://gateway".into(),
        };
        assert!(registry.for_loaded(&loaded_http).is_some());
    }

    #[test]
    fn registry_shares_one_database_source_per_path() {
        let registry = SourceRegistry::default();
        let first = registry.for_cli(&CliSource::Db("a.duckdb".into()));
        let loaded = registry
            .for_loaded(&GraphSourceKind::LiveStore {
                path: "a.duckdb".into(),
            })
            .expect("database sources serve documents");
        assert!(Arc::ptr_eq(&first, &loaded));
        assert!(!first.is_writable());

        let other = registry.for_cli(&CliSource::Db("b.duckdb".into()));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn snapshot_expands_from_the_full_view_and_serves_no_documents() {
        let source = SnapshotSource::new("graph.json");
        let empty = GraphView {
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        let request = ExpandRequest {
            current: &empty,
            full: None,
            selected: "a",
            max_nodes: 10,
            project: None,
            include_tags: false,
        };
        assert_eq!(
            source.expand(&request).unwrap_err(),
            "Expand requires a loaded graph"
        );
        assert!(source
            .expand(&ExpandRequest {
                full: Some(&empty),
                ..request
            })
            .is_ok());
        assert!(source.document(Some("a"), None, None).is_err());
        assert!(source.wiki_catalog(None).is_err());
    }
}
