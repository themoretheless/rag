//! Always-on status bar: mode, seed, counts, caps, layout, and read-only instructions.
//!
//! Spec (EGUI_GRAPH_VIEW §6.3 / §2.6): always show source · seed · depth · nodes/edges ·
//! truncated · frozen/layout. Caps are code constants (300 nodes / 2000 draw edges).

use egui::Ui;

use crate::adapter::UiGraph;
use crate::load::{GraphSourceKind, UI_HARD_MAX_NODES, UI_MAX_DRAW_EDGES};

/// User-facing open mode for the status line (`snapshot` | `db` | `none`).
fn mode_label(source: Option<&GraphSourceKind>) -> &'static str {
    match source {
        Some(GraphSourceKind::LiveStore { .. }) => "db",
        Some(GraphSourceKind::SnapshotFile { .. } | GraphSourceKind::VaultGraphJson { .. }) => {
            "snapshot"
        }
        Some(GraphSourceKind::HttpService { .. }) => "http",
        None => "none",
    }
}

/// Draw the bottom status bar (read-only inspector chrome).
///
/// Shows: mode, detailed source, file/mtime, seed, depth, node/edge counts vs hard caps,
/// layout freeze, optional truncation banner, and brief interaction instructions.
#[allow(clippy::too_many_arguments)]
pub fn draw_status(
    ui: &mut Ui,
    source: Option<&GraphSourceKind>,
    seed_label: Option<&str>,
    depth: u32,
    graph: Option<&UiGraph>,
    layout_frozen: bool,
    truncated: bool,
    banner: Option<&str>,
) {
    ui.horizontal_wrapped(|ui| {
        // Mode: snapshot | db (XOR open path; dual-live write forbidden).
        ui.strong(format!("mode={}", mode_label(source)));

        let src = source.map(|s| s.label()).unwrap_or("none");
        ui.label(format!("source={src}"));

        if let Some(s) = source {
            match s {
                GraphSourceKind::HttpService { base } => {
                    ui.label(format!("url={base}"));
                }
                _ => {
                    let path = s.path();
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_else(|| path.to_str().unwrap_or("?"));
                    ui.label(format!("file={name}"));
                    if let Some(mtime) = s.mtime() {
                        if let Ok(elapsed) = mtime.elapsed() {
                            ui.label(format!("age={}s", elapsed.as_secs()));
                        }
                    }
                }
            }
        }

        ui.separator();
        ui.label(format!("seed={}", seed_label.unwrap_or("-")));
        ui.separator();
        ui.label(format!("depth={depth}"));
        ui.separator();

        // Counts + hard caps (EGUI_GRAPH_VIEW §8.1).
        let (n, e) = match graph {
            Some(g) => (g.nodes.len(), g.edges.len()),
            None => (0, 0),
        };
        ui.label(format!(
            "nodes={n}/{UI_HARD_MAX_NODES} edges={e}/{UI_MAX_DRAW_EDGES}"
        ));

        let capped = truncated
            || graph.is_some_and(|g| g.truncated_nodes || g.truncated_edges);
        if capped {
            ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "capped");
        }

        ui.separator();
        ui.label(if layout_frozen {
            "layout=RadialLocal/frozen"
        } else {
            "layout=pending"
        });
    });

    // Secondary line: instructions + optional warning banner.
    ui.horizontal_wrapped(|ui| {
        ui.weak("read-only · pan drag · scroll zoom · click select · Expand / Open in toolbar");
        if let Some(msg) = banner {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(230, 180, 80), msg);
        }
    });
}
