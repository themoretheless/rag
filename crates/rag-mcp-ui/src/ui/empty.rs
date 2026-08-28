//! Empty / error banners with one primary action hint.
//!
//! When a graph source is loaded but no seed is set, the canvas must not paint
//! a global hairball: prompt for seed label/id and show loaded topology stats.

use egui::Ui;
use rag_mcp::GraphView;
use std::collections::BTreeMap;

/// Why the central panel is not drawing a local graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyKind {
    /// No `--snapshot` / `--db` (and nothing loaded).
    NoSource,
    /// Source opened but 0 nodes.
    EmptyGraph,
    /// Graph loaded; seed required before RadialLocal paint.
    MissingSeed,
    /// Seed text did not resolve to a node id.
    SeedNotFound,
    /// Snapshot parse / exclusive DB open failed.
    LoadError,
    /// Seed view exists but filters removed every node.
    FiltersEmpty,
    /// Over hard layout cap without a usable seed-BFS sample.
    OverCap,
}

/// Lightweight topology summary for empty-state chrome (not a second domain model).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmptyGraphStats {
    pub nodes: usize,
    pub edges: usize,
    /// Pre-cap node count when the loader truncated (EGUI hard cap 300).
    pub raw_nodes: Option<usize>,
    pub truncated: bool,
    /// Counts by node `kind` (document / tag / stub / entity / …), sorted by key.
    pub nodes_by_kind: BTreeMap<String, usize>,
    /// Counts by edge `rel_type`, sorted by key.
    pub edges_by_rel_type: BTreeMap<String, usize>,
}

impl EmptyGraphStats {
    /// Build stats from a loaded domain [`GraphView`].
    pub fn from_view(view: &GraphView, raw_node_count: usize, truncated: bool) -> Self {
        let mut nodes_by_kind: BTreeMap<String, usize> = BTreeMap::new();
        for n in &view.nodes {
            *nodes_by_kind.entry(n.kind.clone()).or_insert(0) += 1;
        }
        let mut edges_by_rel_type: BTreeMap<String, usize> = BTreeMap::new();
        for e in &view.edges {
            *edges_by_rel_type.entry(e.rel_type.clone()).or_insert(0) += 1;
        }
        let raw_nodes = if truncated && raw_node_count > view.nodes.len() {
            Some(raw_node_count)
        } else {
            None
        };
        Self {
            nodes: view.nodes.len(),
            edges: view.edges.len(),
            raw_nodes,
            truncated,
            nodes_by_kind,
            edges_by_rel_type,
        }
    }
}

/// Draw centered empty / error banner. Pass `stats` when a graph is loaded so
/// MissingSeed / SeedNotFound can show topology counts without painting.
pub fn draw_empty_banner(
    ui: &mut Ui,
    kind: EmptyKind,
    detail: Option<&str>,
    stats: Option<&EmptyGraphStats>,
) {
    let (title, hint) = match kind {
        EmptyKind::NoSource => (
            "Open a snapshot or database",
            "Launch with --http URL, --snapshot PATH, or --db PATH",
        ),
        EmptyKind::EmptyGraph => (
            "Graph is empty",
            "Ingest documents or open another file",
        ),
        EmptyKind::MissingSeed => (
            "Pick a seed node",
            "Enter a seed label or id in the toolbar, then Apply seed (or press Enter). Pass --seed at launch to skip this step.",
        ),
        EmptyKind::SeedNotFound => (
            "No node matches seed",
            "Clear the seed field and retype a node id, display label, or document_id",
        ),
        EmptyKind::LoadError => ("Load failed", "Retry / switch to snapshot"),
        EmptyKind::FiltersEmpty => (
            "Filters hide all nodes",
            "Enable tags/stubs or clear filters, then Rebuild",
        ),
        EmptyKind::OverCap => (
            "Too large for layout",
            "Enter a seed for local BFS (hard cap 300 nodes / 2000 edges)",
        ),
    };

    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.18);
        ui.heading(title);
        if let Some(d) = detail {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::from_rgb(220, 120, 100), d);
        }
        ui.add_space(4.0);
        ui.label(hint);

        // Explicit seed prompt on the two states where a seed is the blocker.
        if matches!(kind, EmptyKind::MissingSeed | EmptyKind::SeedNotFound) {
            ui.add_space(12.0);
            ui.group(|ui| {
                ui.set_max_width(420.0);
                ui.label("Seed accepts:");
                ui.label("  · node id");
                ui.label("  · display label");
                ui.label("  · document_id");
                ui.weak("Local neighbors (depth 1, max 100) load only after a seed resolves.");
            });
        }

        if let Some(s) = stats {
            ui.add_space(16.0);
            draw_loaded_stats(ui, s);
        }
    });
}

/// User action from the no-source / load-error start screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoSourceAction {
    #[default]
    None,
    /// Retry the CLI-provided source (`--http` / `--snapshot` / `--db`).
    Retry,
    /// Connect to the HTTP gateway URL from the input field (no restart).
    Connect,
}

/// Start screen shown when no source is loaded (or the initial load failed).
///
/// Explains the three launch modes, offers Retry for a failed CLI source, and
/// an HTTP URL field + Connect to switch to a live gateway without a restart.
pub fn draw_no_source(
    ui: &mut Ui,
    connect_url: &mut String,
    error: Option<&str>,
    can_retry: bool,
) -> NoSourceAction {
    let mut action = NoSourceAction::None;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.15);
        ui.heading(if error.is_some() {
            "Load failed"
        } else {
            "Open a data source"
        });
        if let Some(err) = error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::from_rgb(220, 120, 100), err);
        }
        ui.add_space(8.0);
        ui.group(|ui| {
            ui.set_max_width(520.0);
            ui.label("Three ways to run:");
            ui.monospace("  rag-mcp-ui --http http://127.0.0.1:7432");
            ui.weak("    live via HTTP gateway (server: RAG_HTTP_BIND=…)");
            ui.monospace("  rag-mcp-ui --snapshot graph.json");
            ui.weak("    read-only Mode C export (safe while MCP holds the DB)");
            ui.monospace("  rag-mcp-ui --db rag.duckdb");
            ui.weak("    exclusive DuckDB open (MCP must not be running)");
        });
        ui.add_space(10.0);
        if ui
            .add_enabled(can_retry, egui::Button::new("Retry"))
            .on_hover_text("Retry the source given on the command line")
            .clicked()
        {
            action = NoSourceAction::Retry;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("HTTP URL");
            ui.add(
                egui::TextEdit::singleline(connect_url)
                    .desired_width(240.0)
                    .hint_text("http://127.0.0.1:7432"),
            );
            if ui.button("Connect").clicked() {
                action = NoSourceAction::Connect;
            }
        });
        ui.weak("Connect switches to the HTTP gateway without restarting.");
    });
    action
}

fn draw_loaded_stats(ui: &mut Ui, s: &EmptyGraphStats) {
    ui.group(|ui| {
        ui.set_max_width(480.0);
        ui.heading("Loaded graph");
        ui.horizontal(|ui| {
            ui.label(format!("nodes={}", s.nodes));
            ui.separator();
            ui.label(format!("edges={}", s.edges));
            if s.truncated {
                ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "capped");
            }
        });
        if let Some(raw) = s.raw_nodes {
            ui.weak(format!(
                "raw nodes {raw} (showing {} after hard cap)",
                s.nodes
            ));
        }
        if !s.nodes_by_kind.is_empty() {
            ui.add_space(4.0);
            ui.label("Nodes by kind");
            ui.horizontal_wrapped(|ui| {
                for (kind, n) in &s.nodes_by_kind {
                    ui.monospace(format!("{kind}:{n}"));
                }
            });
        }
        if !s.edges_by_rel_type.is_empty() {
            ui.add_space(2.0);
            ui.label("Edges by rel_type");
            ui.horizontal_wrapped(|ui| {
                for (rel, n) in &s.edges_by_rel_type {
                    ui.monospace(format!("{rel}:{n}"));
                }
            });
        }
        ui.add_space(4.0);
        ui.weak("Paint starts after seed → local neighbors (RadialLocal). No global layout.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_mcp::{GraphEdge, GraphNode};

    fn node(id: &str, kind: &str, label: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
            document_id: None,
            uri: None,
            resolved: true,
            metadata_json: "{}".into(),
        }
    }

    fn edge(id: &str, s: &str, t: &str, rel: &str) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            source_id: s.into(),
            target_id: t.into(),
            rel_type: rel.into(),
            weight: 1.0,
            context: None,
        }
    }

    #[test]
    fn stats_from_view_counts_kinds_and_rels() {
        let view = GraphView {
            nodes: vec![
                node("a", "document", "A"),
                node("b", "document", "B"),
                node("t", "tag", "topic"),
                node("s", "stub", "Missing"),
            ],
            edges: vec![
                edge("e1", "a", "b", "wikilink"),
                edge("e2", "a", "t", "tagged"),
                edge("e3", "b", "s", "wikilink"),
            ],
        };
        let s = EmptyGraphStats::from_view(&view, 4, false);
        assert_eq!(s.nodes, 4);
        assert_eq!(s.edges, 3);
        assert_eq!(s.nodes_by_kind.get("document"), Some(&2));
        assert_eq!(s.nodes_by_kind.get("tag"), Some(&1));
        assert_eq!(s.edges_by_rel_type.get("wikilink"), Some(&2));
        assert_eq!(s.edges_by_rel_type.get("tagged"), Some(&1));
        assert!(!s.truncated);
        assert_eq!(s.raw_nodes, None);
    }

    #[test]
    fn stats_records_raw_when_truncated() {
        let view = GraphView {
            nodes: vec![node("a", "document", "A")],
            edges: vec![],
        };
        let s = EmptyGraphStats::from_view(&view, 500, true);
        assert_eq!(s.nodes, 1);
        assert!(s.truncated);
        assert_eq!(s.raw_nodes, Some(500));
    }
}
