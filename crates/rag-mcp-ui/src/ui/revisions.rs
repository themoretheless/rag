//! Paginated revision timeline, lazy snapshots, bounded line diff and CAS restore.

use crate::load::DocumentBody;
use crate::revisions::{RestoreRevisionResult, RevisionDiff, RevisionItem};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RevisionsAction {
    #[default]
    None,
    BackToLibrary,
    Refresh,
    LoadMore,
    Select(i64),
    RequestRestore(i64),
    ConfirmRestore(i64),
    CancelRestore,
}

pub struct RevisionsView<'a> {
    pub document_title: &'a str,
    pub document_uri: &'a str,
    pub document_layer: &'a str,
    pub head_revision: Option<i64>,
    pub revisions: &'a [RevisionItem],
    pub total: u64,
    pub next_cursor: Option<&'a str>,
    pub selected_revision: Option<i64>,
    pub snapshot: Option<&'a DocumentBody>,
    pub diff: Option<&'a RevisionDiff>,
    pub restore_result: Option<&'a RestoreRevisionResult>,
    pub confirming_restore: Option<i64>,
    pub timeline_error: Option<&'a str>,
    pub snapshot_error: Option<&'a str>,
    pub diff_error: Option<&'a str>,
    pub loading_timeline: bool,
    pub loading_snapshot: bool,
    pub loading_diff: bool,
    pub loading_restore: bool,
}

pub fn draw_revisions_workspace(ui: &mut egui::Ui, view: RevisionsView<'_>) -> RevisionsAction {
    let mut action = RevisionsAction::None;
    ui.horizontal(|ui| {
        if ui.button("← Library").clicked() {
            action = RevisionsAction::BackToLibrary;
        }
        ui.vertical(|ui| {
            ui.heading("Revision history");
            ui.weak(view.document_title);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            if ui.button("Refresh").clicked() {
                action = RevisionsAction::Refresh;
            }
            if view.loading_timeline || view.loading_snapshot || view.loading_diff {
                ui.spinner();
            }
        });
    });
    ui.weak(view.document_uri);
    ui.add_space(12.0);
    if let Some(error) = view.timeline_error {
        ui.colored_label(egui::Color32::from_rgb(215, 100, 85), error);
        ui.add_space(8.0);
    }
    if let Some(result) = view.restore_result {
        egui::Frame::group(ui.style())
            .fill(egui::Color32::from_rgb(65, 145, 95).gamma_multiply(0.12))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.strong(format!(
                    "Restored revision {} as new head revision {}",
                    result.restored_from_revision, result.revision
                ));
                ui.label(format!(
                    "{} chunks · {} graph edges · {}",
                    result.chunk_count, result.edge_count, result.etag
                ));
            });
        ui.add_space(10.0);
    }

    if let Some(revision) = view.confirming_restore {
        egui::Frame::group(ui.style())
            .fill(egui::Color32::from_rgb(190, 125, 55).gamma_multiply(0.13))
            .stroke(egui::Stroke::new(
                1.0_f32,
                egui::Color32::from_rgb(190, 125, 55),
            ))
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.strong(format!("Restore historical revision {revision}?"));
                ui.label("This creates a new head revision and rebuilds chunks and graph. The current head remains in history. Optimistic concurrency prevents overwriting a newer edit.");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!view.loading_restore, egui::Button::new("Confirm restore"))
                        .clicked()
                    {
                        action = RevisionsAction::ConfirmRestore(revision);
                    }
                    if ui.button("Cancel").clicked() {
                        action = RevisionsAction::CancelRestore;
                    }
                    if view.loading_restore {
                        ui.spinner();
                    }
                });
            });
        ui.add_space(10.0);
    }

    ui.columns(3, |columns| {
        columns[0].strong("Current head");
        columns[0].label(
            view.head_revision
                .map(|revision| format!("Revision {revision}"))
                .unwrap_or_else(|| "Load the document preview first".to_string()),
        );

        columns[0].add_space(12.0);
        columns[0].strong(format!(
            "{} of {} historical snapshots",
            view.revisions.len(),
            view.total
        ));
        columns[0].separator();
        egui::ScrollArea::vertical()
            .id_salt("revision_timeline")
            .show(&mut columns[0], |ui| {
                if view.revisions.is_empty() && !view.loading_timeline {
                    ui.weak("No previous revisions. The first edit will create one.");
                }
                for revision in view.revisions {
                    let selected = view.selected_revision == Some(revision.revision);
                    if ui
                        .selectable_label(
                            selected,
                            format!(
                                "r{} · {}",
                                revision.revision,
                                short_timestamp(&revision.superseded_at)
                            ),
                        )
                        .clicked()
                    {
                        action = RevisionsAction::Select(revision.revision);
                    }
                    ui.weak(summary(revision));
                    ui.add_space(5.0);
                }
                if view.next_cursor.is_some() {
                    ui.add_space(6.0);
                    if ui
                        .add_enabled(
                            !view.loading_timeline,
                            egui::Button::new("Load more revisions"),
                        )
                        .clicked()
                    {
                        action = RevisionsAction::LoadMore;
                    }
                    if view.loading_timeline {
                        ui.spinner();
                    }
                }
            });

        columns[1].strong("Changes to current head");
        columns[1].separator();
        columns[1].set_min_width(320.0);
        columns[2].set_min_width(260.0);
        if let Some(diff) = view.diff {
            columns[1].horizontal_wrapped(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(75, 165, 105),
                    format!("+{} lines", diff.added_lines),
                );
                ui.colored_label(
                    egui::Color32::from_rgb(215, 100, 85),
                    format!("-{} lines", diff.removed_lines),
                );
                if diff.title_changed {
                    ui.label("title changed");
                }
                if diff.metadata_changed {
                    ui.label("metadata changed");
                }
                if diff.placement_changed {
                    ui.label("scope changed");
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("revision_diff")
                .show(&mut columns[1], |ui| {
                    for change in &diff.changes {
                        let (prefix, color) = if change.kind == "added" {
                            ("+", egui::Color32::from_rgb(75, 165, 105))
                        } else {
                            ("-", egui::Color32::from_rgb(215, 100, 85))
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(color, prefix);
                            ui.monospace(format!("{}  {}", change.line, change.content));
                        });
                    }
                    if diff.truncated {
                        ui.weak("Diff is truncated to the first 400 changes.");
                    }
                });
        } else if let Some(error) = view.diff_error {
            columns[1].colored_label(egui::Color32::from_rgb(215, 100, 85), error);
            if let Some(revision) = view.selected_revision {
                if columns[1].button("Retry diff").clicked() {
                    action = RevisionsAction::Select(revision);
                }
            }
        } else if view.loading_diff {
            columns[1].spinner();
            columns[1].weak("Loading diff…");
        } else {
            columns[1].weak("Select a historical revision to inspect its diff.");
        }

        columns[2].strong("Selected snapshot");
        columns[2].separator();
        if let Some(selected) = view.selected_revision.and_then(|selected| {
            view.revisions
                .iter()
                .find(|item| item.revision == selected)
        }) {
            columns[2].label(format!("Revision {}", selected.revision));
            columns[2].weak(short_timestamp(&selected.updated_at));
            columns[2].label(format!(
                "{} · {} · {}",
                selected.layer, selected.kind, selected.status
            ));
            columns[2].weak(format!(
                "{} lines · {} characters",
                selected.content_lines, selected.content_chars
            ));
            columns[2].add_space(8.0);
            if view.loading_snapshot {
                columns[2].spinner();
                columns[2].weak("Loading this snapshot…");
            } else if let Some(error) = view.snapshot_error {
                columns[2].colored_label(egui::Color32::from_rgb(215, 100, 85), error);
                if columns[2].button("Retry snapshot").clicked() {
                    action = RevisionsAction::Select(selected.revision);
                }
            } else if let Some(snapshot) = view.snapshot {
                egui::ScrollArea::vertical()
                    .id_salt("revision_snapshot")
                    .max_height(360.0)
                    .show(&mut columns[2], |ui| {
                        ui.monospace(truncate(&snapshot.content, 6_000));
                    });
            } else {
                columns[2].weak("The body is loaded only when you select a revision.");
            }
            columns[2].add_space(10.0);
            let source_controlled =
                view.document_layer == "raw" || selected.layer.as_str() == "raw";
            if source_controlled {
                egui::Frame::group(columns[2].style())
                    .fill(egui::Color32::from_rgb(190, 125, 55).gamma_multiply(0.10))
                    .inner_margin(10.0)
                    .show(&mut columns[2], |ui| {
                        ui.strong("Source-controlled document");
                        ui.label("Raw revisions cannot be restored here. Restore the source file, then sync it to create a new indexed revision.");
                    });
            } else if columns[2]
                .add_enabled(
                    view.head_revision.is_some()
                        && view.snapshot.is_some()
                        && view.confirming_restore.is_none()
                        && !view.loading_snapshot
                        && !view.loading_restore,
                    egui::Button::new("Restore as new head"),
                )
                .clicked()
            {
                action = RevisionsAction::RequestRestore(selected.revision);
            }
        } else {
            columns[2].weak("Nothing selected.");
        }
    });
    action
}

fn summary(item: &RevisionItem) -> String {
    format!(
        "{} · {} lines · {} chars",
        item.layer, item.content_lines, item.content_chars
    )
}

fn short_timestamp(value: &str) -> String {
    value.strip_suffix('Z').unwrap_or(value).replace('T', " ")
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut value = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    value.push('…');
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_truncation_is_unicode_safe() {
        assert_eq!(truncate("абвгде", 4), "абв…");
    }

    #[test]
    fn timestamps_are_compact() {
        assert_eq!(
            short_timestamp("2026-09-02T12:30:00Z"),
            "2026-09-02 12:30:00"
        );
    }

    #[test]
    fn lean_summary_does_not_need_snapshot_content() {
        let item = RevisionItem {
            document_id: "d1".into(),
            uri: "wiki://one".into(),
            title: "One".into(),
            wing: Some("alpha".into()),
            room: None,
            layer: "wiki".into(),
            kind: "wiki".into(),
            status: "active".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            superseded_at: "2026-09-02T00:00:00Z".into(),
            revision: 2,
            content_chars: 120,
            content_lines: 7,
        };
        assert_eq!(summary(&item), "wiki · 7 lines · 120 chars");
    }
}
