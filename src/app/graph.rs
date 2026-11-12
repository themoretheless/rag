//! `GraphUseCase`: the local neighbourhood of one seed, for both transports.

use std::sync::Arc;

use crate::db::Store;
use crate::error::AppError;
use crate::models::{GraphView, UI_GRAPH_EXPORT_MAX_NODES, pkb_rel_types};
use crate::util::run_blocking;

/// Default BFS depth when the caller sends none.
pub const DEFAULT_NEIGHBORS_DEPTH: u32 = 1;
/// Hard depth cap (one rule on MCP and HTTP).
pub const MAX_NEIGHBORS_DEPTH: u32 = 3;
/// Node cap when the caller sends none.
pub const DEFAULT_NEIGHBORS_MAX_NODES: u32 = 100;

/// Typed neighbours command. MCP `node_id` and HTTP `seed` both map to `seed`.
#[derive(Debug, Clone, Default)]
pub struct NeighborsCommand {
    pub seed: String,
    pub depth: Option<u32>,
    pub max_nodes: Option<u32>,
    /// Explicit relation set; empty or absent means the PKB walk.
    pub rel_types: Option<Vec<String>>,
    pub include_tags: Option<bool>,
    /// Restrict the walk to one project (`wing`).
    pub project: Option<String>,
}

/// Graph read use cases over one store.
#[derive(Clone)]
pub struct GraphUseCase {
    store: Arc<Store>,
}

impl GraphUseCase {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    /// Bounded neighbourhood of a seed.
    ///
    /// Rules (the same on both transports):
    /// - blank seed → `Invalid`;
    /// - the seed resolves as node id, then document id, then node URI, then
    ///   exact label (document nodes first); nothing found → `NotFound`;
    /// - depth defaults to 1 and is clamped to `1..=3`; `max_nodes` defaults
    ///   to 100 and is clamped to the UI export cap;
    /// - with `rel_types` the walk follows exactly those relations, otherwise
    ///   the PKB relation/kind policy of the UI graph (`include_tags` adds tag hubs);
    /// - with `project` the walk stays inside that project (same seed order,
    ///   PKB policy); an empty result is `NotFound`; `rel_types` with
    ///   `project` is `Invalid`.
    pub async fn neighbors(&self, command: NeighborsCommand) -> Result<GraphView, AppError> {
        let seed = command.seed.trim().to_owned();
        if seed.is_empty() {
            return Err(AppError::invalid("seed (or node_id) is required"));
        }
        let depth = command
            .depth
            .unwrap_or(DEFAULT_NEIGHBORS_DEPTH)
            .clamp(1, MAX_NEIGHBORS_DEPTH);
        let max_nodes = command
            .max_nodes
            .unwrap_or(DEFAULT_NEIGHBORS_MAX_NODES)
            .clamp(1, UI_GRAPH_EXPORT_MAX_NODES);
        let include_tags = command.include_tags.unwrap_or(false);
        let project = command
            .project
            .map(|project| project.trim().to_owned())
            .filter(|project| !project.is_empty());
        let rel_types = command.rel_types.filter(|rels| !rels.is_empty());
        if project.is_some() && rel_types.is_some() {
            return Err(AppError::invalid(
                "rel_types cannot be combined with project; the project walk uses the PKB policy",
            ));
        }
        let store = self.store.clone();
        run_blocking("neighbors", move || {
            let graph = store.graph();
            if let Some(project) = project {
                // Same seed order as the global walk (node id, document id,
                // node URI, exact label); the project walk then keeps only
                // nodes of that project. When the globally resolved node lies
                // outside the project (an ambiguous label), the project's own
                // id/document/label match is tried with the raw seed.
                let resolved = graph.resolve_seed(&seed)?;
                let mut view = match resolved {
                    Some(node) => graph.export_project_neighbors_for_ui(
                        &project,
                        &node.id,
                        depth,
                        max_nodes,
                        include_tags,
                    )?,
                    None => GraphView::default(),
                };
                if view.nodes.is_empty() {
                    view = graph.export_project_neighbors_for_ui(
                        &project,
                        &seed,
                        depth,
                        max_nodes,
                        include_tags,
                    )?;
                }
                if view.nodes.is_empty() {
                    return Err(AppError::not_found(format!(
                        "no graph node for seed '{seed}' in project '{project}'"
                    )));
                }
                return Ok(view);
            }
            let node = graph.resolve_seed(&seed)?.ok_or_else(|| {
                AppError::not_found(format!(
                    "no graph node for seed '{seed}' (try id, document_id, or exact label)"
                ))
            })?;
            match rel_types {
                Some(rels) => graph.neighbors_filtered(&node.id, depth, max_nodes, Some(&rels)),
                None => {
                    let view = graph.export_pkb_neighbors_for_ui(
                        &node.id,
                        depth,
                        max_nodes,
                        include_tags,
                    )?;
                    if view.nodes.is_empty() {
                        // The seed exists but sits outside the PKB node kinds
                        // (for example a tag hub without include_tags): walk
                        // the PKB relations from it directly.
                        let rels = pkb_rel_types(include_tags);
                        return graph.neighbors_filtered(&node.id, depth, max_nodes, Some(&rels));
                    }
                    Ok(view)
                }
            }
        })
        .await
    }
}
