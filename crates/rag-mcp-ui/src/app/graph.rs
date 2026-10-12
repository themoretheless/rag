//! Graph topology: seed resolution, content for selection, layout rebuild, expand.

use super::*;

impl GraphApp {
    pub(super) fn apply_seed_from_input(&mut self) {
        self.seed_error = None;
        let seed = self.seed_input.trim();
        if seed.is_empty() {
            self.seed_id = None;
            self.rebuild_ui_graph();
            return;
        }
        let Some(full) = self.full_view.as_ref() else {
            self.seed_error = Some("Граф не загружен".into());
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
    pub(super) fn load_content_for_selected(&mut self) {
        self.content_error = None;
        let Some(sel) = self.selected.clone() else {
            self.content_error = Some("Ничего не выбрано".into());
            return;
        };
        let Some(g) = self.ui_graph.as_ref() else {
            self.content_error = Some("Представление графа недоступно".into());
            return;
        };
        let Some(node) = g.nodes.iter().find(|n| n.id == sel) else {
            self.content_error = Some("Выбранного узла нет в текущем представлении".into());
            return;
        };
        if node.document_id.is_none() && node.uri.is_none() {
            self.content_error = Some(format!(
                "У узла «{}» нет документа ({} / unresolved stub?)",
                node.label, node.kind
            ));
            return;
        }
        let (doc_id, uri, label) = (
            node.document_id.clone(),
            node.uri.clone(),
            node.label.clone(),
        );

        let Some(source) = self.available_load_source() else {
            self.content_error = Some(
                "В snapshot-режиме нет содержимого документов; используйте --http или --db".into(),
            );
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

    pub(super) fn rebuild_ui_graph(&mut self) {
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
            if self.graph_lens == GraphLens::Neighborhood {
                self.local_view = None;
                self.local_truncated = false;
                self.ui_graph = None;
                self.positions.clear();
                self.layout_ready = false;
                self.need_fit = true;
                return;
            }
            let pkb_rels_only = !matches!(
                self.graph_lens,
                GraphLens::Architecture | GraphLens::Provenance
            );
            let opts = AdaptOptions {
                lens: self.graph_lens,
                seed_id: None,
                show_tags: self.show_tags,
                show_stubs: self.show_stubs,
                pkb_rels_only,
                wing: nonempty(&self.filter_wing),
                room: nonempty(&self.filter_room),
                show_all_nodes: true,
            };
            let overview = adapt(full, &opts);
            overview_grid_place(&overview, &mut self.positions);
            self.local_view = None;
            self.local_truncated = false;
            self.ui_graph = Some(overview);
            self.need_fit = true;
            self.layout_ready = true;
            return;
        };

        let local = local_neighbors_bounded(full, seed, self.depth, self.max_nodes as usize);
        self.local_truncated = local.capped;
        self.apply_local_topology(local.view, true);
    }

    /// Adapt `local`, update layout. `reset_layout` re-runs RadialLocal; expand uses false.
    pub(super) fn apply_local_topology(&mut self, local: GraphView, reset_layout: bool) {
        let seed = self.seed_id.clone();
        let pkb_rels_only = !matches!(
            self.graph_lens,
            GraphLens::Architecture | GraphLens::Provenance
        );
        let opts = AdaptOptions {
            lens: self.graph_lens,
            seed_id: seed.clone(),
            show_tags: self.show_tags,
            show_stubs: self.show_stubs,
            pkb_rels_only,
            wing: nonempty(&self.filter_wing),
            room: nonempty(&self.filter_room),
            show_all_nodes: false,
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
    pub(super) fn expand_selected(&mut self) {
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
            self.expand_note = Some(format!(
                "Раскрытие недоступно: уже достигнут max_nodes ({max_n})"
            ));
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
            self.expand_note = Some("Для раскрытия нужен загруженный граф".into());
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
}
