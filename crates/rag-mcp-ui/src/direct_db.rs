//! Direct read-only DuckDB access for `--db` mode (feature `direct-db`).
//!
//! Every function here opens the database through `rag_mcp::Store`. Without
//! the feature, `direct_db_disabled.rs` provides the same signatures and the
//! UI builds without the server crate (no DuckDB, axum or rmcp).

#![allow(clippy::wildcard_imports)]

use crate::load::*;
use rag_mcp::db::store::WikiPageMetaFilter;
use rag_mcp::{GraphFilter, GraphView, Store, UI_GRAPH_EXPORT_MAX_NODES};
use std::path::Path;

/// List wiki pages from a strictly read-only DuckDB open (metadata only).
pub fn fetch_wiki_list_db(
    db_path: &Path,
    project: Option<&str>,
) -> Result<Vec<WikiPageMeta>, String> {
    let store = Store::open_read_only(db_path).map_err(|e| format!("open db read-only: {e}"))?;
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
pub fn fetch_document_db(
    db_path: &Path,
    document_id: Option<&str>,
    uri: Option<&str>,
) -> Result<DocumentBody, String> {
    let store = Store::open_read_only(db_path).map_err(|e| {
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
        layer: doc.layer,
        kind: doc.kind,
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
pub fn load_live_db(
    path: &Path,
    seed: Option<&str>,
    depth: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<LoadedGraph, String> {
    if !path.exists() {
        return Err(format!(
            "cannot open database read-only: path does not exist: {}. Use --http for the live gateway or --snapshot for an exported view.",
            path.display()
        ));
    }

    let store = Store::open_read_only(path).map_err(|e| {
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
/// Mode A (`--db`) only. Re-opens DuckDB briefly without migration. Project
/// scope is applied inside Store before the node cap.
pub fn expand_neighbors_store(
    db_path: &Path,
    current: &GraphView,
    selected_id: &str,
    max_nodes: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<GraphView, String> {
    let max_nodes = max_nodes.max(1);
    if !db_path.exists() {
        return Err(format!(
            "cannot expand: database does not exist: {}. Use --snapshot if the agent holds the file.",
            db_path.display()
        ));
    }
    let store = Store::open_read_only(db_path).map_err(|e| {
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
pub fn fetch_backlinks_db(
    path: &Path,
    document_id: &str,
    project: Option<&str>,
) -> Result<Vec<BacklinkItem>, String> {
    let store = rag_mcp::Store::open_read_only(path)
        .map_err(|error| format!("open {} read-only for backlinks: {error}", path.display()))?;
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
