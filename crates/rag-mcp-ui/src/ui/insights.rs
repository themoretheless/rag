//! Read-only observability workspaces built from data already loaded by the
//! native client. They never synthesize benchmark or runtime values.

use crate::load::ActivityEvent;
use crate::operations::OperationsSnapshot;
use crate::search::SearchResults;

use super::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EvaluationAction {
    #[default]
    None,
    OpenSearch,
    RefreshTelemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelsAction {
    #[default]
    None,
    Refresh,
    OpenMaintenance,
}

pub fn draw_evaluation_workspace(
    ui: &mut egui::Ui,
    results: Option<&SearchResults>,
    activity: &[ActivityEvent],
    activity_error: Option<&str>,
    loading: bool,
) -> EvaluationAction {
    let mut action = EvaluationAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            screen_header(
                ui,
                "Оценка извлечения",
                "Наблюдаемость текущей сессии; benchmark-метрики появляются только после воспроизводимого eval-прогона.",
                |ui| {
                    if ui.button("Обновить telemetry").clicked() {
                        action = EvaluationAction::RefreshTelemetry;
                    }
                    if ui.button("Открыть поиск").clicked() {
                        action = EvaluationAction::OpenSearch;
                    }
                },
            );

            let latencies = search_latencies(activity);
            let p50 = percentile(&latencies, 0.50);
            let p95 = percentile(&latencies, 0.95);
            let last_mode = results.map(|value| value.request.mode.as_str());
            let last_hits = results.map(|value| value.items.len() as u64);

            ui.columns(4, |columns| {
                metric_text(&mut columns[0], "Последний режим", last_mode.unwrap_or("—"), "применённый search");
                metric_text(
                    &mut columns[1],
                    "Последние хиты",
                    &last_hits.map(compact_count).unwrap_or_else(|| "—".into()),
                    "текущий снимок",
                );
                metric_text(
                    &mut columns[2],
                    "p50 API",
                    &p50.map(milliseconds).unwrap_or_else(|| "—".into()),
                    "по журналу search",
                );
                metric_text(
                    &mut columns[3],
                    "p95 API",
                    &p95.map(milliseconds).unwrap_or_else(|| "—".into()),
                    "по журналу search",
                );
            });

            ui.add_space(12.0);
            ui.columns(2, |columns| {
                theme::card().show(&mut columns[0], |ui| {
                    ui.strong("Последний retrieval-срез");
                    ui.add_space(8.0);
                    if let Some(results) = results {
                        key_value(ui, "Запрос", &results.request.query);
                        key_value(ui, "Режим", &results.request.mode);
                        key_value(ui, "top_k", &results.request.top_k.to_string());
                        key_value(
                            ui,
                            "Проект",
                            results.request.wing.as_deref().unwrap_or("все"),
                        );
                        key_value(ui, "Хиты", &results.items.len().to_string());
                        if let Some(top) = results.items.first() {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("ЛУЧШИЙ ХИТ").small().color(theme::FAINT));
                            ui.strong(if top.document_title.trim().is_empty() {
                                &top.document_uri
                            } else {
                                &top.document_title
                            });
                            ui.label(
                                egui::RichText::new(format!("score {:.3}", top.score))
                                    .monospace()
                                    .color(theme::L1),
                            );
                        }
                    } else {
                        empty_note(ui, "Запросов в этой сессии ещё нет", "Откройте Поиск и выполните lex, vec или hybrid запрос.");
                    }
                });

                theme::card().show(&mut columns[1], |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("Latency · успешный /v1/search");
                        if loading {
                            ui.spinner();
                        }
                    });
                    ui.add_space(8.0);
                    if let Some(error) = activity_error {
                        ui.colored_label(theme::DANGER, error);
                    }
                    if latencies.is_empty() {
                        empty_note(ui, "Нет измерений search", "Это не ноль: telemetry пока не содержит завершённых search-вызовов.");
                    } else {
                        for value in latencies.iter().rev().take(10).rev() {
                            let ratio = (*value as f32 / p95.unwrap_or(*value).max(1.0) as f32)
                                .clamp(0.03, 1.0);
                            ui.horizontal(|ui| {
                                ui.add_sized([58.0, 18.0], egui::Label::new(milliseconds(*value)));
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2((ui.available_width() - 6.0).max(12.0), 6.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, 3.0, theme::SURFACE_2);
                                let fill = egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(rect.width() * ratio, rect.height()),
                                );
                                ui.painter().rect_filled(fill, 3.0, theme::L1);
                            });
                        }
                        ui.add_space(5.0);
                        ui.weak(format!("{} успешных HTTP-измерений в журнале", latencies.len()));
                    }
                });
            });

            ui.add_space(12.0);
            theme::inset().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("EVAL").monospace().color(theme::L1));
                    ui.strong("Recall@k, MRR и nDCG здесь не подменяются telemetry.");
                    ui.label("Для них нужен versioned dataset и отдельный eval binary; отсутствие history отображается как «—».");
                });
            });
            ui.add_space(20.0);
        });
    action
}

pub fn draw_models_workspace(
    ui: &mut egui::Ui,
    snapshot: Option<&OperationsSnapshot>,
    error: Option<&str>,
    loading: bool,
    snapshot_stale: bool,
) -> ModelsAction {
    let mut action = ModelsAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            screen_header(
                ui,
                "Runtime и индекс",
                "Факты из /v1/status и /v1/doctor; конкретная LLM или embedding-модель не угадывается без отдельного API.",
                |ui| {
                    if ui.button("Обновить").clicked() {
                        action = ModelsAction::Refresh;
                    }
                    if ui.button("Диагностика и бэкап").clicked() {
                        action = ModelsAction::OpenMaintenance;
                    }
                },
            );

            theme::inset().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("GLOBAL STORE")
                            .monospace()
                            .strong()
                            .color(theme::L3),
                    );
                    ui.label(
                        "Числа /v1/status и /v1/doctor относятся ко всей базе и не зависят от выбранного проекта.",
                    );
                });
            });
            ui.add_space(10.0);

            if loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Читаю /v1/status и /v1/doctor…");
                });
                ui.add_space(8.0);
            }
            if let Some(error) = error {
                theme::inset().show(ui, |ui| {
                    ui.colored_label(theme::DANGER, error);
                    if snapshot.is_some() {
                        ui.weak("Ниже показан последний успешный snapshot.");
                    }
                });
                ui.add_space(10.0);
            }
            if snapshot_stale && error.is_none() {
                theme::inset().show(ui, |ui| {
                    ui.colored_label(
                        theme::WARN,
                        "Последний runtime-снимок устарел: свежая проверка не завершилась успешно.",
                    );
                });
                ui.add_space(10.0);
            }

            let Some(snapshot) = snapshot else {
                if !loading {
                    theme::card().show(ui, |ui| {
                        empty_note(ui, "Runtime пока не загружен", "Подключите HTTP gateway или повторите обновление.");
                    });
                }
                return;
            };
            let status = &snapshot.status;
            let doctor = &snapshot.doctor;

            ui.columns(4, |columns| {
                metric_text(&mut columns[0], "Backend", &status.backend, "адаптер хранения");
                metric_text(
                    &mut columns[1],
                    "Схема",
                    &format!("v{}", status.schema_version),
                    if doctor.schema_ok { "совпадает" } else { "несовместима" },
                );
                metric_text(
                    &mut columns[2],
                    "FTS",
                    if status.fts_ready { "готов" } else { "не готов" },
                    "лексический индекс",
                );
                metric_text(
                    &mut columns[3],
                    "Эмбеддинги",
                    if status.embedding_manifest_match { "совпадает" } else { "расхождение" },
                    "контракт manifest",
                );
            });

            ui.add_space(12.0);
            ui.columns(3, |columns| {
                pipeline_card(
                    &mut columns[0],
                    "01 · Corpus",
                    theme::L0,
                    &[
                        ("Документы", compact_count(status.document_count)),
                        ("Raw", compact_count(status.raw_count)),
                        ("Корни", yes_no(status.ingest_roots_configured)),
                    ],
                );
                pipeline_card(
                    &mut columns[1],
                    "02 · Retrieval",
                    theme::L1,
                    &[
                        ("Чанки", compact_count(status.chunk_count)),
                        ("FTS", yes_no(status.fts_ready)),
                        ("Готов", yes_no(status.ready_for_search)),
                    ],
                );
                pipeline_card(
                    &mut columns[2],
                    "03 · Knowledge",
                    theme::L3,
                    &[
                        ("Wiki", compact_count(status.wiki_count)),
                        ("Индекс", format!("{:.1}%", status.index_coverage.clamp(0.0, 1.0) * 100.0)),
                        ("Граф", format!("{} / {}", compact_count(status.node_count), compact_count(status.edge_count))),
                    ],
                );
            });

            ui.add_space(12.0);
            ui.columns(2, |columns| {
                theme::card().show(&mut columns[0], |ui| {
                    ui.strong("Контракт безопасности");
                    ui.add_space(8.0);
                    status_row(ui, "schema", doctor.schema_ok, format!("{} = {}", doctor.schema_version, doctor.expected_schema_version));
                    status_row(ui, "embedding manifest", doctor.embed_ok, if doctor.embed_ok { "совпадает" } else { "проверьте конфигурацию" });
                    status_row(ui, "целостность связей", doctor.relational_integrity_ok, format!("{} orphan chunks · {} orphan edges", doctor.orphan_chunks, doctor.orphan_edges));
                    status_row(ui, "документы с чанками", doctor.documents_without_chunks == 0, format!("{} без чанков", doctor.documents_without_chunks));
                    status_row(ui, "WAL", !doctor.wal_too_large, format!("{} / {} MiB", doctor.wal_bytes / 1_048_576, doctor.wal_warn_bytes / 1_048_576));
                });
                theme::card().show(&mut columns[1], |ui| {
                    ui.strong("Единственный писатель");
                    ui.add_space(8.0);
                    ui.label("В HTTP-режиме native-клиент направляет чтения и мутации через gateway. Запуски с --db и --snapshot остаются изолированными read-only режимами просмотра.");
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("DB PATH").small().color(theme::FAINT));
                    theme::inset().show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&status.db_path).monospace()).selectable(true).wrap());
                    });
                    ui.add_space(8.0);
                    status_row(ui, "gateway doctor", doctor.ok, if doctor.ok { "в норме" } else { "требует внимания" });
                });
            });
            ui.add_space(20.0);
        });
    action
}

fn screen_header(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    actions: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.heading(title);
            ui.label(egui::RichText::new(subtitle).color(theme::MUTED));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), actions);
    });
    ui.add_space(16.0);
}

fn metric_text(ui: &mut egui::Ui, label: &str, value: &str, detail: &str) {
    theme::card().show(ui, |ui| {
        ui.set_min_height(75.0);
        ui.label(
            egui::RichText::new(label.to_uppercase())
                .small()
                .color(theme::FAINT),
        );
        ui.label(egui::RichText::new(value).size(21.0).strong());
        ui.label(egui::RichText::new(detail).small().color(theme::MUTED));
    });
}

fn pipeline_card(ui: &mut egui::Ui, title: &str, color: egui::Color32, rows: &[(&str, String)]) {
    theme::card().show(ui, |ui| {
        ui.label(egui::RichText::new(title).monospace().strong().color(color));
        ui.add_space(8.0);
        for (label, value) in rows {
            key_value(ui, label, value);
        }
    });
}

fn status_row(ui: &mut egui::Ui, label: &str, ok: bool, detail: impl std::fmt::Display) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(if ok { "OK" } else { "!" }).color(if ok {
                theme::OK
            } else {
                theme::DANGER
            }),
        );
        ui.strong(label);
        ui.label(egui::RichText::new(detail.to_string()).color(theme::MUTED));
    });
}

fn key_value(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(key).color(theme::FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).monospace());
        });
    });
}

fn empty_note(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.strong(title);
    ui.label(egui::RichText::new(detail).color(theme::MUTED));
}

fn search_latencies(activity: &[ActivityEvent]) -> Vec<f64> {
    let mut values: Vec<_> = activity
        .iter()
        .filter(|event| {
            event.kind.eq_ignore_ascii_case("http")
                && event
                    .status
                    .is_some_and(|status| (200..400).contains(&status))
                && event
                    .action
                    .split_ascii_whitespace()
                    .find(|part| part.starts_with('/'))
                    .and_then(|path| path.split('?').next())
                    .is_some_and(|path| path == "/v1/search")
        })
        .filter_map(|event| event.elapsed_ms)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect();
    values.sort_by(f64::total_cmp);
    values
}

fn percentile(sorted: &[f64], quantile: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let position = (sorted.len() - 1) as f64 * quantile.clamp(0.0, 1.0);
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let weight = position - lower as f64;
    Some(sorted[lower] + (sorted[upper] - sorted[lower]) * weight)
}

fn milliseconds(value: f64) -> String {
    format!("{value:.0} мс")
}

fn compact_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn yes_no(value: bool) -> String {
    if value { "да" } else { "нет" }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_latency_is_not_reported_as_zero() {
        assert_eq!(percentile(&[], 0.95), None);
    }

    #[test]
    fn latency_cohort_is_successful_http_search_only() {
        let event = |kind: &str, action: &str, status: Option<u16>, elapsed_ms: Option<f64>| {
            ActivityEvent {
                seq: 1,
                at: "2026-09-03T00:00:00Z".into(),
                kind: kind.into(),
                client: None,
                action: action.into(),
                status,
                elapsed_ms,
                request_id: None,
            }
        };
        let activity = [
            event("http", "POST /v1/search", Some(200), Some(12.0)),
            event("http", "POST /v1/search", Some(500), Some(99.0)),
            event("mcp", "search", None, Some(8.0)),
            event("http", "GET /v1/search-wiki", Some(200), Some(7.0)),
        ];
        assert_eq!(search_latencies(&activity), vec![12.0]);
    }

    #[test]
    fn percentile_interpolates_between_samples() {
        let values = [10.0, 20.0, 30.0, 90.0];
        assert!((percentile(&values, 0.5).unwrap() - 25.0).abs() < 1e-9);
        assert!((percentile(&values, 0.95).unwrap() - 81.0).abs() < 1e-9);
        assert_eq!(percentile(&[103.0, 611.0], 0.5), Some(357.0));
    }
}
