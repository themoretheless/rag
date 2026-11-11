//! Direct read-only DuckDB access for `--db` mode (feature `direct-db`).
//!
//! [`DbSource`] opens the database once through `rag_store_duckdb::Store` and keeps that
//! read-only connection for every later call. Without the feature the UI
//! builds without the server crate (no DuckDB, axum or rmcp).

use std::fmt;
use std::path::PathBuf;
use std::sync::Mutex;

use rag_store_duckdb::db::store::WikiPageMetaFilter;
use rag_store_duckdb::models::{GraphFilter, GraphView, UI_GRAPH_EXPORT_MAX_NODES};
use rag_store_duckdb::Store;

use crate::cli::ExportArgs;
use crate::dto::{
    sort_wiki_pages, BacklinkItem, DocumentBody, ExportResult, GraphSourceKind, LoadedGraph,
    WikiPageMeta, WikiPutRequest,
};
use crate::snapshot::{write_graph_snapshot, write_snapshot_bytes, SnapshotEnvelopeOut};
use crate::source::{DocumentSource, ExpandRequest, READ_ONLY_SOURCE};
use crate::topology::{merge_graph_views, pkb_default_rel_types, UI_HARD_MAX_NODES};

/// Read-only DuckDB file (Mode A). Direct writes are forbidden; mutations go
/// through the HTTP gateway.
pub struct DbSource {
    path: PathBuf,
    /// Opened on first use and then reused; `Store` clones share one connection.
    store: Mutex<Option<Store>>,
}

impl fmt::Debug for DbSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbSource")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl DbSource {
    /// Bind to a database path. Nothing is opened until the first call.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            store: Mutex::new(None),
        }
    }

    /// The shared read-only store, opening it on first use. `describe` words
    /// the open failure for the calling operation.
    fn store(&self, describe: impl FnOnce(&dyn fmt::Display) -> String) -> Result<Store, String> {
        let mut slot = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(store) = slot.as_ref() {
            return Ok(store.clone());
        }
        let store = Store::open_read_only(&self.path).map_err(|error| describe(&error))?;
        *slot = Some(store.clone());
        Ok(store)
    }
}

impl DocumentSource for DbSource {
    fn load_graph(
        &self,
        seed: Option<&str>,
        depth: u32,
        project: Option<&str>,
        include_tags: bool,
    ) -> Result<LoadedGraph, String> {
        load_live_db(self, seed, depth, project, include_tags)
    }

    fn wiki_catalog(&self, project: Option<&str>) -> Result<Vec<WikiPageMeta>, String> {
        fetch_wiki_list_db(self, project)
    }

    fn document(
        &self,
        doc_id: Option<&str>,
        uri: Option<&str>,
        _label: Option<&str>,
    ) -> Result<DocumentBody, String> {
        fetch_document_db(self, doc_id, uri)
    }

    fn backlinks(
        &self,
        document_id: &str,
        project: Option<&str>,
    ) -> Result<Vec<BacklinkItem>, String> {
        fetch_backlinks_db(self, document_id, project)
    }

    fn save_wiki(&self, _req: &WikiPutRequest) -> Result<DocumentBody, String> {
        Err(READ_ONLY_SOURCE.into())
    }

    fn expand(&self, request: &ExpandRequest<'_>) -> Result<GraphView, String> {
        expand_neighbors_store(
            self,
            request.current,
            request.selected,
            request.max_nodes,
            request.project,
            request.include_tags,
        )
    }
}

/// List wiki pages from a strictly read-only DuckDB open (metadata only).
fn fetch_wiki_list_db(
    source: &DbSource,
    project: Option<&str>,
) -> Result<Vec<WikiPageMeta>, String> {
    let store = source.store(|e| format!("open db read-only: {e}"))?;
    let (items, _) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            wing: project.map(str::to_owned),
            ..WikiPageMetaFilter::default()
        })
        .map_err(|e| e.to_string())?;
    let mut pages: Vec<WikiPageMeta> = items
        .into_iter()
        .map(|d| WikiPageMeta {
            id: d.id,
            uri: d.uri,
            slug: d.slug,
            title: d.title,
            kind: d.kind,
            summary: d.summary,
            category: d.category,
            revision: d.revision,
            etag: Some(d.etag),
            updated_at: Some(d.updated_at),
        })
        .collect();
    sort_wiki_pages(&mut pages);
    Ok(pages)
}

/// Load document body via read-only DuckDB open (Mode A `--db` only).
fn fetch_document_db(
    source: &DbSource,
    document_id: Option<&str>,
    uri: Option<&str>,
) -> Result<DocumentBody, String> {
    let db_path = source.path.as_path();
    let store = source.store(|e| {
        format!(
            "cannot open DB {} read-only: {e}. Prefer --http while MCP holds the file.",
            db_path.display()
        )
    })?;
    let doc = if let Some(id) = document_id.map(str::trim).filter(|s| !s.is_empty()) {
        store
            .get_document(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("document id '{id}' not found"))?
    } else if let Some(u) = uri.map(str::trim).filter(|s| !s.is_empty()) {
        store
            .find_by_uri(u)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("document uri '{u}' not found"))?
    } else {
        return Err("need document_id or uri".into());
    };
    let rev = doc.revision;
    let etag = doc.etag();
    Ok(DocumentBody {
        id: doc.id,
        uri: doc.uri,
        title: doc.title,
        layer: doc.layer.to_string(),
        kind: doc.kind.to_string(),
        content: doc.content,
        content_hash: doc.content_hash,
        updated_at: Some(doc.updated_at.to_rfc3339()),
        revision: Some(rev),
        etag: Some(etag),
    })
}

/// Read-only live Store open (Mode A): `Store::open_read_only` +
/// [`Store::export_graph_for_ui`].
///
/// PKB defaults centralized on Store: `rel_types = [wikilink, related]` and
/// `max_nodes = 100,000` ([`UI_GRAPH_EXPORT_MAX_NODES`]); tag inclusion follows the
/// UI toggle.
/// Fails clearly if the path cannot be opened (missing file, lock, corrupt DB).
/// Direct writes are unsupported; if a writer lock blocks the read-only open,
/// switch to `--http` or `--snapshot` (Mode C).
///
/// With a seed, query the Store neighborhood directly so an exact id,
/// document_id, or label remains reachable even when it falls beyond the
/// bounded default export. Without a seed, return the bounded project/global
/// topology and let the UI focus locally.
fn load_live_db(
    source: &DbSource,
    seed: Option<&str>,
    depth: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<LoadedGraph, String> {
    let path = source.path.as_path();
    if !path.exists() {
        return Err(format!(
            "cannot open database read-only: path does not exist: {}. Use --http for the live gateway or --snapshot for an exported view.",
            path.display()
        ));
    }

    let store = source.store(|e| {
        format!(
            "cannot open DuckDB read-only at {}: {}. If MCP (or another process) holds an incompatible lock, use --http or --snapshot Mode C. Direct writes are forbidden.",
            path.display(),
            e
        )
    })?;

    let project = project.map(str::trim).filter(|project| !project.is_empty());
    let seed = seed.map(str::trim).filter(|seed| !seed.is_empty());
    let view = match (project, seed) {
        (Some(project), Some(seed)) => store.export_project_neighbors_for_ui(
            project,
            seed,
            depth,
            UI_GRAPH_EXPORT_MAX_NODES,
            include_tags,
        ),
        (None, Some(seed)) => {
            store.export_pkb_neighbors_for_ui(seed, depth, UI_GRAPH_EXPORT_MAX_NODES, include_tags)
        }
        (Some(project), None) => store.export_project_graph_for_ui(
            project,
            Some(UI_GRAPH_EXPORT_MAX_NODES),
            include_tags,
        ),
        (None, None) => store.export_graph_for_ui(Some(UI_GRAPH_EXPORT_MAX_NODES), include_tags),
    }
    .map_err(|e| {
        format!(
            "UI graph export failed on {}: {}. Check schema / graph tables.",
            path.display(),
            e
        )
    })?;

    let (total_nodes, truncated) = if project.is_some() || seed.is_some() {
        // The project predicate is applied in Store/SQL before the cap. Store's
        // bounded project export does not expose an uncapped count, so reaching
        // the limit is conservatively reported as potentially truncated.
        let visible = view.nodes.len();
        (
            visible,
            visible >= UI_HARD_MAX_NODES || visible as u32 >= UI_GRAPH_EXPORT_MAX_NODES,
        )
    } else {
        let total = store
            .graph_stats()
            .map(|stats| stats.total_nodes as usize)
            .unwrap_or(view.nodes.len());
        (
            total,
            total > view.nodes.len()
                || view.nodes.len() >= UI_HARD_MAX_NODES
                || view.nodes.len() as u32 >= UI_GRAPH_EXPORT_MAX_NODES,
        )
    };

    let projects = store
        .list_projects()
        .map_err(|e| format!("list projects from {}: {e}", path.display()))?
        .into_iter()
        .map(|project| project.project_id.as_str().to_owned())
        .collect();

    Ok(LoadedGraph {
        view,
        projects: Some(projects),
        source: GraphSourceKind::LiveStore {
            path: path.to_path_buf(),
        },
        truncated,
        raw_node_count: total_nodes,
        health: None,
    })
}

/// Export GraphView topology from DuckDB to a Mode C snapshot file.
///
/// Uses `Store::open_read_only` and never migrates the source. Topology only: no
/// positions. Default `max_nodes` matches MCP `get_graph` (500). If the writer's
/// lock prevents a direct read, use the gateway or export from a copy.
pub fn export_graph_snapshot(args: &ExportArgs) -> Result<ExportResult, String> {
    if !args.db.exists() {
        return Err(format!(
            "cannot open database read-only: path does not exist: {}. Use a current backup or export through the gateway.",
            args.db.display()
        ));
    }

    let store = Store::open_read_only(&args.db).map_err(|e| {
        format!(
            "cannot open DuckDB read-only at {}: {}. If MCP (or another process) holds an incompatible lock, use the HTTP gateway or copy the DB.",
            args.db.display(),
            e
        )
    })?;

    let max_nodes = args.max_nodes.max(1);
    let view = if args.pkb
        && args.kinds.is_none()
        && args.seed_ids.is_none()
        && args.rel_types.is_none()
    {
        // Centralized Store helper (tags off; §7.1 set = wikilink, related, wiki semantic).
        store
            .export_graph_for_ui(Some(max_nodes), false)
            .map_err(|e| {
                format!(
                    "export_graph_for_ui failed on {}: {}. Check schema / graph tables.",
                    args.db.display(),
                    e
                )
            })?
    } else {
        let rel_types = if args.pkb {
            Some(pkb_default_rel_types())
        } else {
            args.rel_types.clone()
        };
        let filter = GraphFilter {
            kinds: args.kinds.clone(),
            rel_types,
            seed_ids: args.seed_ids.clone(),
            max_nodes: Some(max_nodes),
        };
        store.get_graph_view(filter).map_err(|e| {
            format!(
                "get_graph_view failed on {}: {}. Check schema / graph tables.",
                args.db.display(),
                e
            )
        })?
    };

    let node_count = view.nodes.len();
    let edge_count = view.edges.len();
    let truncated = node_count >= max_nodes as usize;

    if args.envelope {
        let env = SnapshotEnvelopeOut {
            version: 1,
            source: "duckdb",
            db_path: args.db.display().to_string(),
            graph: &view,
        };
        let json = serde_json::to_string_pretty(&env)
            .map_err(|e| format!("serialize snapshot envelope: {e}"))?;
        write_snapshot_bytes(&args.output, json.as_bytes())?;
    } else {
        write_graph_snapshot(&args.output, &view)?;
    }

    Ok(ExportResult {
        output: args.output.clone(),
        node_count,
        edge_count,
        truncated,
    })
}

/// Expand neighbors via read-only Store BFS (`Store::neighbors`), depth 1 from
/// the selected node, merge into `current` under `max_nodes`.
///
/// Mode A (`--db`) only. Reuses the source's read-only store. Project
/// scope is applied inside Store before the node cap.
fn expand_neighbors_store(
    source: &DbSource,
    current: &GraphView,
    selected_id: &str,
    max_nodes: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<GraphView, String> {
    let db_path = source.path.as_path();
    let max_nodes = max_nodes.max(1);
    if !db_path.exists() {
        return Err(format!(
            "cannot expand: database does not exist: {}. Use --snapshot if the agent holds the file.",
            db_path.display()
        ));
    }
    let store = source.store(|e| {
        format!(
            "cannot open DuckDB read-only for expand at {}: {}. Use --http while the gateway owns the writer, or use a snapshot.",
            db_path.display(),
            e
        )
    })?;
    // depth=1: one hop (depth+1 from selected). Cap matches UI max_nodes.
    let project = project.map(str::trim).filter(|project| !project.is_empty());
    let extra = match project {
        Some(project) => {
            store.export_project_neighbors_for_ui(project, selected_id, 1, max_nodes, include_tags)
        }
        None => store.export_pkb_neighbors_for_ui(selected_id, 1, max_nodes, include_tags),
    }
    .map_err(|e| {
        format!(
            "Store neighbor expansion failed on {}: {e}",
            db_path.display()
        )
    })?;
    if extra.nodes.is_empty() && !current.nodes.iter().any(|n| n.id == selected_id) {
        return Err(format!(
            "expand: no node id “{selected_id}” in store (or isolated with no neighbors)"
        ));
    }
    // The bounded initial view cannot be an edge source for newly loaded
    // nodes. Union the Store neighborhood so each newcomer keeps its connector.
    Ok(merge_graph_views(current, &extra, None, max_nodes as usize))
}

/// Wiki backlinks for a document from a read-only DuckDB open (`--db`).
fn fetch_backlinks_db(
    source: &DbSource,
    document_id: &str,
    project: Option<&str>,
) -> Result<Vec<BacklinkItem>, String> {
    let path = source.path.as_path();
    let store = source
        .store(|error| format!("open {} read-only for backlinks: {error}", path.display()))?;
    let rows = match project.map(str::trim).filter(|project| !project.is_empty()) {
        Some(project) => store.wiki_backlinks_for_document_in_wing(document_id, project),
        None => store.wiki_backlinks_for_document(document_id),
    }
    .map_err(|error| format!("load backlinks from {}: {error}", path.display()))?;
    Ok(rows
        .into_iter()
        .map(|(label, id)| BacklinkItem { label, id })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::load_snapshot_path;
    use crate::topology::test_support::{edge, node};
    use rag_store_duckdb::models::{Document, GraphEdge};
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;
    use std::time::SystemTime;

    fn load_live_db(
        path: &Path,
        seed: Option<&str>,
        depth: u32,
        project: Option<&str>,
        include_tags: bool,
    ) -> Result<LoadedGraph, String> {
        DbSource::new(path).load_graph(seed, depth, project, include_tags)
    }

    fn expand_neighbors_store(
        path: &Path,
        current: &GraphView,
        selected: &str,
        max_nodes: u32,
        project: Option<&str>,
        include_tags: bool,
    ) -> Result<GraphView, String> {
        DbSource::new(path).expand(&ExpandRequest {
            current,
            full: None,
            selected,
            max_nodes,
            project,
            include_tags,
        })
    }

    #[test]
    fn one_read_only_store_is_reused_across_calls() {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rag-mcp-ui-shared-store-{suffix}"));
        fs::create_dir_all(&dir).expect("create temp directory");
        let db = dir.join("rag.duckdb");
        drop(Store::open(&db).expect("create database"));

        let source = DbSource::new(&db);
        assert!(source.store.lock().unwrap().is_none());
        source.wiki_catalog(None).expect("catalog");
        assert!(source.store.lock().unwrap().is_some());
        // A second call must not need to reopen the file.
        source.backlinks("missing", None).expect("backlinks");
        let save = source.save_wiki(&WikiPutRequest {
            id: String::new(),
            slug: "page".into(),
            uri: None,
            title: "Page".into(),
            content: String::new(),
            if_match_revision: None,
            if_match_etag: None,
        });
        assert_eq!(save.unwrap_err(), READ_ONLY_SOURCE);
        drop(source);
        fs::remove_dir_all(&dir).expect("remove temp directory");
    }

    #[cfg(unix)]
    #[test]
    fn direct_db_loader_uses_read_only_store_and_preserves_database_bytes() {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rag-mcp-ui-read-only-{suffix}"));
        fs::create_dir_all(&dir).expect("create temp directory");
        let db = dir.join("read-only.duckdb");
        let store = Store::open(&db).expect("create fixture store");
        store
            .upsert_graph_node(&node("read-only-node", "Read only"))
            .expect("seed graph node");
        drop(store);

        let original_permissions = fs::metadata(&db).expect("DB metadata").permissions();
        let mut read_only_permissions = original_permissions.clone();
        read_only_permissions.set_readonly(true);
        fs::set_permissions(&db, read_only_permissions).expect("make fixture read-only");
        let bytes_before = fs::read(&db).expect("DB bytes before UI load");
        let modified_before = fs::metadata(&db)
            .expect("DB metadata before UI load")
            .modified()
            .expect("DB mtime before UI load");

        let loaded = load_live_db(&db, None, 1, None, false);
        let bytes_after = fs::read(&db).expect("DB bytes after UI load");
        let modified_after = fs::metadata(&db)
            .expect("DB metadata after UI load")
            .modified()
            .expect("DB mtime after UI load");
        fs::set_permissions(&db, original_permissions).expect("restore DB permissions");

        let loaded = loaded.expect("direct UI read-only load");
        assert_eq!(loaded.view.nodes.len(), 1);
        assert_eq!(bytes_after, bytes_before);
        assert_eq!(modified_after, modified_before);
        assert!(!PathBuf::from(format!("{}.wal", db.display())).exists());
        fs::remove_dir_all(&dir).expect("remove temp directory");
    }

    #[test]
    fn export_graph_snapshot_from_store() {
        let dir = std::env::temp_dir().join(format!("rag-mcp-ui-export-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let db = dir.join("t.duckdb");
        let out = dir.join("graph.json");
        let _ = fs::remove_file(&db);
        let _ = fs::remove_file(&out);

        let store = Store::open(&db).expect("open temp store");
        store
            .upsert_graph_node(&node("n1", "Note One"))
            .expect("upsert node");
        store
            .upsert_graph_node(&node("n2", "Note Two"))
            .expect("upsert node");
        store
            .insert_graph_edges(&[edge("e1", "n1", "n2")])
            .expect("insert edge");
        drop(store);

        let args = ExportArgs {
            db: db.clone(),
            output: out.clone(),
            max_nodes: 500,
            rel_types: None,
            pkb: false,
            kinds: None,
            seed_ids: None,
            envelope: false,
        };
        let res = export_graph_snapshot(&args).expect("export");
        assert_eq!(res.node_count, 2);
        assert_eq!(res.edge_count, 1);
        assert!(!res.truncated);

        let loaded = load_snapshot_path(&out).expect("reload snapshot");
        assert_eq!(loaded.view.nodes.len(), 2);
        assert_eq!(loaded.view.edges.len(), 1);

        // envelope form
        let out_env = dir.join("graph.envelope.json");
        let args_env = ExportArgs {
            output: out_env.clone(),
            envelope: true,
            ..args
        };
        export_graph_snapshot(&args_env).expect("export envelope");
        let env_loaded = load_snapshot_path(&out_env).expect("reload envelope");
        assert_eq!(env_loaded.view.nodes.len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_db_project_filter_is_applied_before_ui_export() {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rag-mcp-ui-project-{suffix}"));
        fs::create_dir_all(&dir).expect("create temp directory");
        let db = dir.join("project.duckdb");
        let store = Store::open(&db).expect("open temp store");
        for (id, wing) in [("alpha-doc", "alpha"), ("beta-doc", "beta")] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file:///{wing}/{id}.md"),
                    title: format!("{wing} document"),
                    content: wing.into(),
                    wing: Some(wing.into()),
                    ..Document::default()
                })
                .expect("insert document");
            let mut graph_node = node(&format!("{wing}-node"), wing);
            graph_node.document_id = Some(id.into());
            store
                .upsert_graph_node(&graph_node)
                .expect("insert document node");
        }
        drop(store);

        let loaded = load_live_db(&db, None, 1, Some("alpha"), false).expect("project export");
        assert_eq!(loaded.view.nodes.len(), 1);
        assert_eq!(
            loaded.view.nodes[0].document_id.as_deref(),
            Some("alpha-doc")
        );
        assert!(!loaded
            .view
            .nodes
            .iter()
            .any(|node| node.document_id.as_deref() == Some("beta-doc")));

        fs::remove_dir_all(&dir).expect("remove temp directory");
    }

    #[test]
    fn live_db_exact_seed_loads_focused_neighbors() {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rag-mcp-ui-seed-cap-{suffix}"));
        fs::create_dir_all(&dir).expect("create temp directory");
        let db = dir.join("seed.duckdb");
        let store = Store::open(&db).expect("open temp store");
        for index in 0..(UI_HARD_MAX_NODES as u32 + 20) {
            store
                .upsert_graph_node(&node(
                    &format!("early-{index:03}"),
                    &format!("A {index:03}"),
                ))
                .expect("insert early graph node");
        }
        store
            .upsert_graph_node(&node("late-seed", "Z exact seed"))
            .expect("insert late seed");
        store
            .upsert_graph_node(&node("late-neighbor", "Z neighbor"))
            .expect("insert late neighbor");
        store
            .insert_graph_edges(&[edge("late-edge", "late-seed", "late-neighbor")])
            .expect("link late seed");
        drop(store);

        let focused = load_live_db(&db, Some("late-seed"), 1, None, false).expect("focused export");
        assert!(focused.view.nodes.iter().any(|node| node.id == "late-seed"));
        assert!(focused
            .view
            .nodes
            .iter()
            .any(|node| node.id == "late-neighbor"));
        assert!(focused.view.edges.iter().any(|edge| edge.id == "late-edge"));

        fs::remove_dir_all(&dir).expect("remove temp directory");
    }

    #[test]
    fn direct_db_focused_paths_apply_tag_and_relation_filters_before_merge() {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rag-mcp-ui-focused-tags-{suffix}"));
        fs::create_dir_all(&dir).expect("create temp directory");
        let db = dir.join("focused.duckdb");
        let store = Store::open(&db).expect("open temp store");
        let mut tag = node("tag", "Tag");
        tag.kind = "tag".into();
        store.upsert_graph_node(&node("seed", "Seed")).unwrap();
        store.upsert_graph_node(&tag).unwrap();
        store
            .upsert_graph_node(&node("hidden", "Hidden tunnel target"))
            .unwrap();
        store
            .insert_graph_edges(&[
                GraphEdge {
                    id: "tagged".into(),
                    source_id: "seed".into(),
                    target_id: "tag".into(),
                    rel_type: "tagged".into(),
                    weight: 1.0,
                    context: None,
                },
                GraphEdge {
                    id: "tunnel".into(),
                    source_id: "seed".into(),
                    target_id: "hidden".into(),
                    rel_type: "tunnel".into(),
                    weight: 1.0,
                    context: None,
                },
            ])
            .unwrap();
        drop(store);

        let tags_off = load_live_db(&db, Some("Seed"), 1, None, false).expect("focused tags off");
        assert_eq!(
            tags_off
                .view
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["seed"])
        );
        assert!(tags_off.view.edges.is_empty());

        let tags_on = load_live_db(&db, Some("Seed"), 1, None, true).expect("focused tags on");
        assert!(tags_on.view.nodes.iter().any(|node| node.id == "tag"));
        assert!(tags_on
            .view
            .edges
            .iter()
            .any(|edge| edge.rel_type == "tagged"));
        assert!(!tags_on.view.nodes.iter().any(|node| node.id == "hidden"));

        let current = GraphView {
            nodes: vec![node("seed", "Seed")],
            edges: Vec::new(),
        };
        let expanded_off = expand_neighbors_store(&db, &current, "seed", 10, None, false)
            .expect("expand tags off");
        assert!(!expanded_off.nodes.iter().any(|node| node.id == "tag"));
        let expanded_on =
            expand_neighbors_store(&db, &current, "seed", 10, None, true).expect("expand tags on");
        assert!(expanded_on.nodes.iter().any(|node| node.id == "tag"));
        assert!(expanded_on
            .edges
            .iter()
            .any(|edge| edge.rel_type == "tagged"));

        fs::remove_dir_all(&dir).expect("remove temp directory");
    }

    #[test]
    fn expand_neighbors_store_is_project_scoped_and_keeps_connector_edge() {
        let dir = std::env::temp_dir().join(format!("rag-mcp-ui-expand-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let db = dir.join("t.duckdb");
        let _ = fs::remove_file(&db);

        let store = Store::open(&db).expect("open");
        for (id, wing) in [
            ("doc-a", "alpha"),
            ("doc-b", "alpha"),
            ("doc-c", "alpha"),
            ("doc-x", "beta"),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file:///{wing}/{id}.md"),
                    title: id.into(),
                    content: id.into(),
                    wing: Some(wing.into()),
                    ..Document::default()
                })
                .unwrap();
        }
        for (id, label, document_id) in [
            ("a", "A", "doc-a"),
            ("b", "B", "doc-b"),
            ("c", "C", "doc-c"),
            ("x", "X", "doc-x"),
        ] {
            let mut graph_node = node(id, label);
            graph_node.document_id = Some(document_id.into());
            store.upsert_graph_node(&graph_node).unwrap();
        }
        store
            .insert_graph_edges(&[
                edge("e1", "a", "b"),
                edge("e2", "b", "c"),
                edge("cross-project", "b", "x"),
            ])
            .unwrap();
        drop(store);

        let current = GraphView {
            nodes: vec![node("a", "A"), node("b", "B")],
            edges: vec![edge("e1", "a", "b")],
        };
        let expanded = expand_neighbors_store(&db, &current, "b", 100, Some("alpha"), false)
            .expect("expand store");
        let ids: HashSet<_> = expanded.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"));
        assert!(!ids.contains("x"));
        assert!(expanded.edges.iter().any(|edge| edge.id == "e2"));
        assert!(!expanded.edges.iter().any(|edge| edge.id == "cross-project"));

        let _ = fs::remove_dir_all(&dir);
    }
}
