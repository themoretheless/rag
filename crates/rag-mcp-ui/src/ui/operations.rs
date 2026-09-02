//! Operations workspace: observable jobs and safe one-writer maintenance.

use crate::operations::{
    BackupRequest, JobSnapshot, MaintenanceResult, OperationsSnapshot, SyncJobRequest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperationsTab {
    #[default]
    Activity,
    Jobs,
    Maintenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OperationsAction {
    #[default]
    None,
    RefreshHealth,
    RefreshJobs,
    StartSync(SyncJobRequest),
    CancelJob(String),
    Checkpoint,
    Backup(BackupRequest),
}

#[derive(Debug, Clone, Default)]
pub struct SyncJobForm {
    pub path: String,
    pub remove_deleted: bool,
    pub room: String,
    /// Empty means gateway default. Parsed as MiB.
    pub max_file_mib: String,
}

#[derive(Debug, Clone)]
pub struct BackupForm {
    pub path: String,
    pub dry_run: bool,
    pub overwrite: bool,
}

impl Default for BackupForm {
    fn default() -> Self {
        Self {
            path: String::new(),
            dry_run: true,
            overwrite: false,
        }
    }
}

pub fn draw_jobs(
    ui: &mut egui::Ui,
    jobs: &[JobSnapshot],
    form: &mut SyncJobForm,
    project: Option<&str>,
    error: Option<&str>,
    loading: bool,
) -> OperationsAction {
    let mut action = OperationsAction::None;
    let mut validation_error = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("Background jobs");
                ui.weak("Long-running writes stay in the gateway's single writer lane.");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if ui
                    .add_enabled(!loading, egui::Button::new("Refresh"))
                    .clicked()
                {
                    action = OperationsAction::RefreshJobs;
                }
                if loading {
                    ui.spinner();
                }
            });
        });
        ui.add_space(12.0);
        egui::Frame::group(ui.style())
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.strong("Sync a source tree");
                ui.weak("Files are preflighted first; unchanged healthy documents avoid extraction and embedding.");
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut form.path)
                            .hint_text("Allowlisted source directory")
                            .desired_width(330.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut form.room)
                            .hint_text("Room (optional)")
                            .desired_width(130.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut form.max_file_mib)
                            .hint_text("Max MiB")
                            .desired_width(75.0),
                    );
                    ui.checkbox(&mut form.remove_deleted, "Remove deleted");
                    if ui
                        .add_enabled(
                            can_start_sync(loading, &form.path),
                            egui::Button::new("Start sync"),
                        )
                        .clicked()
                    {
                        match sync_request(form, project) {
                            Ok(request) => action = OperationsAction::StartSync(request),
                            Err(error) => validation_error = Some(error),
                        }
                    }
                });
                if let Some(error) = validation_error.as_deref() {
                    ui.colored_label(egui::Color32::from_rgb(215, 100, 85), error);
                }
                if let Some(project) = project {
                    ui.weak(format!("New documents will be scoped to project “{project}”."));
                }
            });

        if let Some(error) = error {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::from_rgb(215, 100, 85), error);
        }
        ui.add_space(14.0);
        if jobs.is_empty() && !loading {
            ui.weak("No retained jobs yet.");
        }
        for job in jobs {
            job_card(ui, job, loading, &mut action);
            ui.add_space(8.0);
        }
    });
    action
}

pub fn draw_maintenance(
    ui: &mut egui::Ui,
    snapshot: Option<&OperationsSnapshot>,
    backup: &mut BackupForm,
    last_result: Option<&MaintenanceResult>,
    error: Option<&str>,
    loading: bool,
) -> OperationsAction {
    let mut action = OperationsAction::None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("Health & maintenance");
                ui.weak("Diagnostics and writes run through the live gateway; the UI never opens DuckDB.");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if ui.button("Refresh diagnostics").clicked() {
                    action = OperationsAction::RefreshHealth;
                }
                if loading {
                    ui.spinner();
                }
            });
        });
        if let Some(error) = error {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::from_rgb(215, 100, 85), error);
        }
        ui.add_space(12.0);
        if let Some(snapshot) = snapshot {
            health_cards(ui, snapshot);
        } else if loading {
            ui.spinner();
        }

        ui.add_space(18.0);
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style())
                .inner_margin(14.0)
                .show(&mut columns[0], |ui| {
                    ui.strong("Checkpoint");
                    ui.label("Flush the DuckDB WAL and report the main file size delta.");
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(!loading, egui::Button::new("Run checkpoint"))
                        .clicked()
                    {
                        action = OperationsAction::Checkpoint;
                    }
                });
            egui::Frame::group(columns[1].style())
                .inner_margin(14.0)
                .show(&mut columns[1], |ui| {
                    ui.strong("Verified backup");
                    ui.label("Destination must be inside a configured allowlisted root.");
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut backup.path)
                            .hint_text("/allowlisted/path/rag-backup.duckdb")
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut backup.dry_run, "Dry run");
                        ui.checkbox(&mut backup.overwrite, "Overwrite existing");
                        let label = if backup.dry_run {
                            "Validate backup"
                        } else {
                            "Create backup"
                        };
                        if ui
                            .add_enabled(
                                !loading && !backup.path.trim().is_empty(),
                                egui::Button::new(label),
                            )
                            .clicked()
                        {
                            action = OperationsAction::Backup(BackupRequest {
                                path: backup.path.trim().to_string(),
                                dry_run: backup.dry_run,
                                overwrite: backup.overwrite,
                            });
                        }
                    });
                });
        });

        if let Some(result) = last_result {
            ui.add_space(14.0);
            egui::CollapsingHeader::new(format!("{} result", result.operation))
                .default_open(true)
                .show(ui, |ui| {
                    let pretty = serde_json::to_string_pretty(&result.report)
                        .unwrap_or_else(|_| result.report.to_string());
                    ui.monospace(pretty);
                });
        }
        ui.add_space(20.0);
    });
    action
}

fn health_cards(ui: &mut egui::Ui, snapshot: &OperationsSnapshot) {
    let status = &snapshot.status;
    let doctor = &snapshot.doctor;
    ui.columns(4, |columns| {
        health_metric(
            &mut columns[0],
            "Search",
            if doctor.ready_for_search {
                "Ready"
            } else {
                "Not ready"
            },
            doctor.ready_for_search,
        );
        health_metric(
            &mut columns[1],
            "Integrity",
            if doctor.relational_integrity_ok {
                "Clean"
            } else {
                "Attention"
            },
            doctor.relational_integrity_ok,
        );
        health_metric(
            &mut columns[2],
            "Schema",
            &format!(
                "{} / {}",
                doctor.schema_version, doctor.expected_schema_version
            ),
            doctor.schema_ok,
        );
        health_metric(
            &mut columns[3],
            "WAL",
            &format!("{} MiB", doctor.wal_bytes / 1_048_576),
            !doctor.wal_too_large,
        );
    });
    ui.add_space(10.0);
    egui::Frame::group(ui.style())
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} · schema {}",
                    status.backend, status.schema_version
                ));
                ui.separator();
                ui.label(if status.fts_ready {
                    "FTS ready"
                } else {
                    "FTS not ready"
                });
                ui.separator();
                ui.label(format!("{} documents", status.document_count));
                ui.separator();
                ui.label(format!("{} chunks", status.chunk_count));
                ui.separator();
                ui.label(format!("{} graph nodes", status.node_count));
                ui.separator();
                ui.label(format!("{} graph edges", status.edge_count));
                ui.separator();
                ui.weak(&status.db_path);
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "raw={} · wiki={}",
                    status.raw_count, status.wiki_count
                ));
                ui.separator();
                ui.label(format!("wiki index {:.0}%", status.index_coverage * 100.0));
                ui.separator();
                ui.label(format!("uncompiled raw={}", status.uncompiled_raw_count));
                ui.separator();
                ui.label(if status.embedding_manifest_match {
                    "embedding manifest matches"
                } else {
                    "embedding manifest mismatch"
                });
                ui.separator();
                ui.label(if status.ready_for_search {
                    "status search-ready"
                } else {
                    "status not search-ready"
                });
                ui.separator();
                ui.label(if status.ingest_roots_configured {
                    "ingest roots configured"
                } else {
                    "no ingest roots"
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(if doctor.fts_ready {
                    "doctor FTS ok"
                } else {
                    "doctor FTS failed"
                });
                ui.separator();
                ui.label(if doctor.embed_ok {
                    "embedding dimensions ok"
                } else {
                    "embedding dimensions mismatch"
                });
                ui.separator();
                ui.label(format!(
                    "WAL warning at {} MiB",
                    doctor.wal_warn_bytes / 1_048_576
                ));
                ui.separator();
                ui.label(format!("unscoped={}", doctor.unscoped_documents));
            });
            if let Some(hint) = doctor.repair_hint.as_deref() {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(215, 145, 55), hint);
            }
            if doctor.documents_without_chunks > 0
                || doctor.orphan_chunks > 0
                || doctor.orphan_document_nodes > 0
                || doctor.orphan_edges > 0
            {
                ui.add_space(6.0);
                ui.colored_label(
                    egui::Color32::from_rgb(215, 100, 85),
                    format!(
                        "missing chunks={} · orphan chunks={} · orphan nodes={} · orphan edges={}",
                        doctor.documents_without_chunks,
                        doctor.orphan_chunks,
                        doctor.orphan_document_nodes,
                        doctor.orphan_edges
                    ),
                );
            }
        });
}

fn job_card(ui: &mut egui::Ui, job: &JobSnapshot, loading: bool, action: &mut OperationsAction) {
    egui::Frame::group(ui.style())
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                status_badge(ui, &job.status);
                ui.strong(&job.request.path);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            can_cancel_job(loading, job.can_cancel()),
                            egui::Button::new("Cancel"),
                        )
                        .clicked()
                    {
                        *action = OperationsAction::CancelJob(job.id.clone());
                    }
                    ui.monospace(short_id(&job.id));
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.weak(format!("{} · created {}", job.kind, short_timestamp(&job.created_at)));
                if let Some(started) = job.started_at.as_deref() {
                    ui.separator();
                    ui.weak(format!("started {}", short_timestamp(started)));
                }
                if let Some(finished) = job.finished_at.as_deref() {
                    ui.separator();
                    ui.weak(format!("finished {}", short_timestamp(finished)));
                }
            });
            if let Some(progress) = &job.progress {
                let fraction = progress_fraction(progress.processed_files, progress.total_files);
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .text(format!(
                            "{} · {}/{} files",
                            phase_label(&progress.phase),
                            progress.processed_files,
                            progress.total_files
                        )),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.weak(format!(
                        "+{} ~{} ={} -{} · {} errors",
                        progress.added,
                        progress.updated,
                        progress.skipped,
                        progress.deleted,
                        progress.errors
                    ));
                    if let Some(path) = progress.current_path.as_deref() {
                        ui.separator();
                        ui.weak(path);
                    }
                    ui.separator();
                    ui.weak(format!(
                        "preflight={} extracted={} embedded={}",
                        progress.counters.preflight,
                        progress.counters.extracted,
                        progress.counters.embedded
                    ));
                });
            }
            if let Some(report) = &job.report {
                ui.label(format!(
                    "Completed: +{} updated={} skipped={} deleted={} errors={} · preflight={} extracted={} embedded={}",
                    report.added_count,
                    report.updated_count,
                    report.skipped_count,
                    report.deleted_count,
                    report.error_count,
                    report.counters.preflight,
                    report.counters.extracted,
                    report.counters.embedded
                ));
                for sample in report.error_samples.iter().take(3) {
                    ui.colored_label(
                        egui::Color32::from_rgb(215, 100, 85),
                        format!("{}: {}", sample.path, sample.error),
                    );
                }
            }
            if let Some(error) = job.error.as_deref() {
                ui.colored_label(egui::Color32::from_rgb(215, 100, 85), error);
            }
        });
}

fn can_start_sync(loading: bool, path: &str) -> bool {
    !loading && !path.trim().is_empty()
}

fn can_cancel_job(loading: bool, job_can_cancel: bool) -> bool {
    !loading && job_can_cancel
}

fn sync_request(form: &SyncJobForm, project: Option<&str>) -> Result<SyncJobRequest, String> {
    let max_file_bytes = if form.max_file_mib.trim().is_empty() {
        None
    } else {
        let mib = form
            .max_file_mib
            .trim()
            .parse::<u64>()
            .map_err(|_| "Max MiB must be a positive integer".to_string())?;
        if mib == 0 {
            return Err("Max MiB must be a positive integer".to_string());
        }
        Some(
            mib.checked_mul(1_048_576)
                .ok_or_else(|| "Max MiB is too large".to_string())?,
        )
    };
    Ok(SyncJobRequest {
        path: form.path.trim().to_string(),
        remove_deleted: form.remove_deleted,
        wing: project.map(str::to_string),
        room: clean(&form.room),
        max_file_bytes,
    })
}

fn clean(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn progress_fraction(processed: usize, total: usize) -> f32 {
    if total == 0 {
        0.0
    } else {
        (processed as f32 / total as f32).clamp(0.0, 1.0)
    }
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn short_timestamp(value: &str) -> String {
    value.get(..19).unwrap_or(value).replace('T', " ")
}

fn health_metric(ui: &mut egui::Ui, label: &str, value: &str, healthy: bool) {
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.weak(label);
            ui.colored_label(
                if healthy {
                    egui::Color32::from_rgb(75, 165, 105)
                } else {
                    egui::Color32::from_rgb(215, 100, 85)
                },
                egui::RichText::new(value).strong(),
            );
        });
}

fn status_badge(ui: &mut egui::Ui, status: &str) {
    ui.colored_label(status_color(status), format!("● {}", status_label(status)));
}

fn status_color(status: &str) -> egui::Color32 {
    match status {
        "succeeded" => egui::Color32::from_rgb(75, 165, 105),
        "failed" => egui::Color32::from_rgb(215, 100, 85),
        "completed_with_errors" => egui::Color32::from_rgb(220, 150, 65),
        "cancelled" => egui::Color32::from_rgb(140, 140, 145),
        _ => egui::Color32::from_rgb(75, 130, 195),
    }
}

fn status_label(status: &str) -> String {
    status.replace('_', " ")
}

fn phase_label(phase: &str) -> String {
    match phase {
        "scanning" => "Scanning files".to_string(),
        "syncing" => "Indexing files".to_string(),
        "removing_deleted" => "Removing deleted files".to_string(),
        "refreshing_fts" => "Preparing search".to_string(),
        "completed" => "Completed".to_string(),
        "cancelled" => "Cancelled".to_string(),
        other => sentence_case(other.replace('_', " ")),
    }
}

fn sentence_case(value: String) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Working".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_fraction_is_bounded() {
        assert_eq!(progress_fraction(0, 0), 0.0);
        assert_eq!(progress_fraction(5, 10), 0.5);
        assert_eq!(progress_fraction(12, 10), 1.0);
    }

    #[test]
    fn sync_form_converts_mib_and_project_scope() {
        let request = sync_request(
            &SyncJobForm {
                path: " /sources ".to_string(),
                room: " docs ".to_string(),
                max_file_mib: "8".to_string(),
                ..SyncJobForm::default()
            },
            Some("alpha"),
        )
        .unwrap();
        assert_eq!(request.path, "/sources");
        assert_eq!(request.wing.as_deref(), Some("alpha"));
        assert_eq!(request.room.as_deref(), Some("docs"));
        assert_eq!(request.max_file_bytes, Some(8 * 1_048_576));
    }

    #[test]
    fn sync_form_rejects_zero_max_mib() {
        let error = sync_request(
            &SyncJobForm {
                path: "/sources".to_string(),
                max_file_mib: "0".to_string(),
                ..SyncJobForm::default()
            },
            Some("alpha"),
        )
        .expect_err("zero would reject every non-empty file");
        assert_eq!(error, "Max MiB must be a positive integer");
    }

    #[test]
    fn short_job_id_never_splits_invalid_boundary() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("123456789"), "12345678");
    }

    #[test]
    fn completed_with_errors_uses_readable_warning_status() {
        assert_eq!(
            status_label("completed_with_errors"),
            "completed with errors"
        );
        assert_eq!(
            status_color("completed_with_errors"),
            egui::Color32::from_rgb(220, 150, 65)
        );
        assert_ne!(
            status_color("completed_with_errors"),
            status_color("running")
        );
        assert_ne!(
            status_color("completed_with_errors"),
            status_color("succeeded")
        );
    }

    #[test]
    fn source_sync_phases_use_user_facing_labels() {
        assert_eq!(phase_label("scanning"), "Scanning files");
        assert_eq!(phase_label("syncing"), "Indexing files");
        assert_eq!(phase_label("removing_deleted"), "Removing deleted files");
        assert_eq!(phase_label("refreshing_fts"), "Preparing search");
        assert_eq!(phase_label("completed"), "Completed");
        assert_eq!(phase_label("cancelled"), "Cancelled");
    }

    #[test]
    fn unknown_source_sync_phase_stays_visible_without_snake_case() {
        assert_eq!(phase_label("hydrating_links"), "Hydrating links");
        assert_eq!(phase_label(""), "Working");
    }

    #[test]
    fn mutation_buttons_are_single_flight() {
        assert!(can_start_sync(false, "/sources"));
        assert!(!can_start_sync(true, "/sources"));
        assert!(!can_start_sync(false, "  "));
        assert!(can_cancel_job(false, true));
        assert!(!can_cancel_job(true, true));
        assert!(!can_cancel_job(false, false));
    }
}
