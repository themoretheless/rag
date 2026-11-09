//! Graph topology: seed resolution, content for selection, layout rebuild, expand.

use super::*;

impl GraphApp {
    /// Seed picker suggestions over `full_view` (label / id / document_id
    /// substring, case-insensitive, first 10), cached per graph revision and
    /// query.
    pub(super) fn seed_suggestions(&mut self, query: &str) -> Vec<(String, String)> {
        if let Some(cached) = &self.graph.seed_suggestions_cache {
            if cached.rev == self.graph.full_view_rev && cached.query == query {
                return cached.items.clone();
            }
        }
        let rev = self.graph.full_view_rev;
        let suggestions = match self.graph.full_view.as_deref() {
            Some(full) => {
                // Lowercase every node once per loaded graph, not once per query.
                if self.graph.seed_search_keys.as_ref().map(|(at, _)| *at) != Some(rev) {
                    self.graph.seed_search_keys = Some((rev, seed_search_keys(full)));
                }
                let keys = self
                    .graph
                    .seed_search_keys
                    .as_ref()
                    .map(|(_, keys)| keys.as_slice())
                    .unwrap_or_default();
                seed_matches(full, keys, query)
            }
            None => Vec::new(),
        };
        self.graph.seed_suggestions_cache = Some(SeedSuggestions {
            rev: self.graph.full_view_rev,
            query: query.to_string(),
            items: suggestions.clone(),
        });
        suggestions
    }

    pub(super) fn apply_seed_from_input(&mut self) {
        self.graph.seed_error = None;
        let seed = self.graph.seed_input.trim();
        if seed.is_empty() {
            self.graph.seed_id = None;
            self.rebuild_ui_graph();
            return;
        }
        let Some(full) = self.graph.full_view.as_deref() else {
            self.graph.seed_error = Some("Граф не загружен".into());
            return;
        };
        match resolve_seed(full, seed) {
            Ok(id) => {
                self.graph.seed_id = Some(id);
                self.rebuild_ui_graph();
            }
            Err(e) => {
                self.graph.seed_id = None;
                self.graph.seed_error = Some(e);
                self.graph.local_view = None;
                self.graph.local_truncated = false;
                self.graph.set_ui_graph(None);
                self.graph.layout_ready = false;
            }
        }
    }

    /// Load full document body for the current selection (wiki or raw).
    pub(super) fn load_content_for_selected(&mut self) {
        self.graph.content_error = None;
        let Some(sel) = self.graph.selected.clone() else {
            self.graph.content_error = Some("Ничего не выбрано".into());
            return;
        };
        let Some(g) = self.graph.ui_graph.as_ref() else {
            self.graph.content_error = Some("Представление графа недоступно".into());
            return;
        };
        let Some(node) = g.nodes.iter().find(|n| n.id == sel) else {
            self.graph.content_error = Some("Выбранного узла нет в текущем представлении".into());
            return;
        };
        if node.document_id.is_none() && node.uri.is_none() {
            self.graph.content_error = Some(format!(
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
            self.graph.content_error = Some(
                "В snapshot-режиме нет содержимого документов; используйте --http или --db".into(),
            );
            return;
        };

        // Keep previous content visible while the worker fetches.
        let seq = self.next_seq();
        self.pending.content.set(seq);
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
        self.graph.expand_note = None;
        self.graph.expanded_dirty = false;
        self.worker.cancel_expand_read();
        // Any in-flight Expand merges old topology into the new local view:
        // invalidate it so the late answer is dropped by seq.
        self.pending.expand.clear();
        let Some(full) = self.graph.full_view.as_deref() else {
            self.graph.local_view = None;
            self.graph.local_truncated = false;
            self.graph.set_ui_graph(None);
            self.graph.layout_ready = false;
            return;
        };

        let Some(seed) = self.graph.seed_id.as_deref() else {
            if self.graph.lens == GraphLens::Neighborhood {
                self.graph.local_view = None;
                self.graph.local_truncated = false;
                self.graph.set_ui_graph(None);
                self.graph.positions.clear();
                self.graph.layout_ready = false;
                self.graph.need_fit = true;
                return;
            }
            let pkb_rels_only = !matches!(
                self.graph.lens,
                GraphLens::Architecture | GraphLens::Provenance
            );
            let opts = AdaptOptions {
                lens: self.graph.lens,
                seed_id: None,
                show_tags: self.graph.show_tags,
                show_stubs: self.graph.show_stubs,
                pkb_rels_only,
                wing: nonempty(&self.scope.wing),
                room: nonempty(&self.scope.room),
                show_all_nodes: true,
            };
            let overview = adapt(full, &opts);
            overview_grid_place(&overview, &mut self.graph.positions);
            self.graph.local_view = None;
            self.graph.local_truncated = false;
            self.graph.set_ui_graph(Some(overview));
            self.graph.need_fit = true;
            self.graph.layout_ready = true;
            return;
        };

        let local =
            local_neighbors_bounded(full, seed, self.graph.depth, self.graph.max_nodes as usize);
        self.graph.local_truncated = local.capped;
        self.apply_local_topology(local.view, true);
    }

    /// Adapt `local`, update layout. `reset_layout` re-runs RadialLocal; expand uses false.
    pub(super) fn apply_local_topology(&mut self, local: GraphView, reset_layout: bool) {
        let seed = self.graph.seed_id.clone();
        let pkb_rels_only = !matches!(
            self.graph.lens,
            GraphLens::Architecture | GraphLens::Provenance
        );
        let opts = AdaptOptions {
            lens: self.graph.lens,
            seed_id: seed.clone(),
            show_tags: self.graph.show_tags,
            show_stubs: self.graph.show_stubs,
            pkb_rels_only,
            wing: nonempty(&self.scope.wing),
            room: nonempty(&self.scope.room),
            show_all_nodes: false,
        };
        let ui_graph = adapt(&local, &opts);
        self.graph.local_view = Some(Arc::new(local));

        if ui_graph.nodes.is_empty() {
            self.graph.set_ui_graph(Some(ui_graph));
            self.graph.layout_ready = false;
            return;
        }

        if reset_layout || self.graph.positions.is_empty() {
            radial_place(&ui_graph, seed.as_deref(), &mut self.graph.positions);
            self.graph.need_fit = true;
        } else {
            // Expand or filter: keep existing positions; place newcomers near neighbors.
            place_missing_near_neighbors(&ui_graph, &mut self.graph.positions);
        }
        self.graph.set_ui_graph(Some(ui_graph));
        self.graph.layout_ready = true;
    }

    /// Expand neighbors of the selection: one hop (depth+1 from that node), merge
    /// into the local view under `max_nodes`. Runs on the worker (EGUI_GRAPH_VIEW
    /// §5.1 / §6.1); does not reseed the local view.
    pub(super) fn expand_selected(&mut self) {
        let Some(sel) = self.graph.selected.clone() else {
            return;
        };
        self.graph.expand_note = None;
        self.graph.seed_error = None;

        // No seed yet: selected becomes seed at depth 1, then paint (no IO).
        if self.graph.seed_id.is_none() {
            self.graph.seed_id = Some(sel.clone());
            self.graph.seed_input = sel;
            self.graph.depth = self.graph.depth.max(1);
            self.rebuild_ui_graph();
            return;
        }

        let max_n = self.graph.max_nodes as usize;
        let current = match self.graph.local_view.clone() {
            Some(v) if !v.nodes.is_empty() => v,
            _ => {
                // Seed set but local empty: rebuild first, then expand if still selected.
                self.rebuild_ui_graph();
                match self.graph.local_view.clone() {
                    Some(v) => v,
                    None => return,
                }
            }
        };

        if current.nodes.len() >= max_n {
            self.graph.expand_note = Some(format!(
                "Раскрытие недоступно: уже достигнут max_nodes ({max_n})"
            ));
            return;
        }

        // A loaded gateway graph expands over HTTP, `--db` through the store,
        // anything else in memory over the full view.
        let source = match (self.source.as_ref(), self.open.source.as_ref()) {
            (Some(GraphSourceKind::HttpService { base }), _) => {
                self.sources.for_cli(&CliSource::Http(base.clone()))
            }
            (_, Some(configured @ CliSource::Db(_))) => self.sources.for_cli(configured),
            _ => crate::source::in_memory(),
        };
        let full = if source.expands_in_memory() {
            self.graph.full_view.clone()
        } else {
            None
        };
        if source.expands_in_memory() && full.is_none() {
            self.graph.expand_note = Some("Для раскрытия нужен загруженный граф".into());
            return;
        }

        // Status depth: at least selected.depth + 1 (one hop outward), capped at 3.
        if let Some(g) = &self.graph.ui_graph {
            if let Some(n) = g.nodes.iter().find(|n| n.id == sel) {
                self.graph.depth = self.graph.depth.max((n.depth + 1).min(3));
            }
        } else {
            self.graph.depth = self.graph.depth.clamp(1, 3);
        }

        let seq = self.next_seq();
        self.pending.expand.set(seq);
        self.worker.send(WorkerCmd::ExpandNeighbors {
            seq,
            selected: sel,
            current,
            full,
            source,
            project: nonempty(&self.scope.wing),
            include_tags: self.graph.show_tags,
            max_nodes: self.graph.max_nodes,
        });
    }
}
