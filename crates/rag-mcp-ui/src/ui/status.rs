//! Compact status bar. Diagnostic details stay available on hover.
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

fn source_detail(source: Option<&GraphSourceKind>) -> String {
    match source {
        Some(GraphSourceKind::HttpService { base }) => format!("HTTP gateway: {base}"),
        Some(source) => {
            let mut detail = format!("{}: {}", source.label(), source.path().display());
            if let Some(age) = source.mtime().and_then(|modified| modified.elapsed().ok()) {
                detail.push_str(&format!(" · updated {}s ago", age.as_secs()));
            }
            detail
        }
        None => "No data source".into(),
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
    pending: bool,
) {
    ui.horizontal_wrapped(|ui| {
        let connected = source.is_some();
        ui.colored_label(
            if connected {
                egui::Color32::from_rgb(90, 190, 125)
            } else {
                egui::Color32::from_rgb(220, 105, 90)
            },
            if connected {
                "● Connected"
            } else {
                "● Offline"
            },
        )
        .on_hover_text(source_detail(source));
        ui.separator();
        ui.label(
            seed_label
                .map(|seed| format!("Focus: {seed}"))
                .unwrap_or_else(|| "No focus selected".into()),
        );
        let (n, e) = match graph {
            Some(g) => (g.nodes.len(), g.edges.len()),
            None => (0, 0),
        };
        ui.separator();
        ui.label(format!("{n} items · {e} connections"));

        let capped = truncated || graph.is_some_and(|g| g.truncated_nodes || g.truncated_edges);
        if capped {
            ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "capped");
        }

        if pending {
            ui.separator();
            ui.spinner();
            ui.weak("Updating…");
        }
        let details = format!(
            "mode={} · depth={} · caps={}/{} · layout={}",
            mode_label(source),
            depth,
            UI_HARD_MAX_NODES,
            UI_MAX_DRAW_EDGES,
            if layout_frozen { "ready" } else { "pending" }
        );
        ui.weak("ⓘ").on_hover_text(details);
        if let Some(msg) = banner {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(230, 180, 80), msg);
        }
    });
}
