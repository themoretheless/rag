//! Product console: one truthful overview of the selected project and gateway.

use crate::load::ActivityEvent;
use crate::operations::OperationsSnapshot;
use crate::product::ProjectHome;

use super::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomeAction {
    #[default]
    None,
    Refresh,
    OpenLibrary,
    OpenSearch,
    OpenGraph,
    OpenWiki,
    OpenAgents,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_project_home(
    ui: &mut egui::Ui,
    selected_project: Option<&str>,
    home: Option<&ProjectHome>,
    operations: Option<&OperationsSnapshot>,
    activity: &[ActivityEvent],
    project_error: Option<&str>,
    operations_error: Option<&str>,
    operations_stale: bool,
    loading: bool,
    http_available: bool,
    wiki_available: bool,
) -> HomeAction {
    let mut action = HomeAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Пульт");
                    let project = selected_project.unwrap_or("все проекты");
                    let source = operations
                        .map(|snapshot| snapshot.status.db_path.as_str())
                        .unwrap_or(if http_available {
                            "HTTP-шлюз проверяется…"
                        } else if wiki_available {
                            "DuckDB · только чтение"
                        } else {
                            "снимок графа · только чтение"
                        });
                    let access = if http_available {
                        "единый процесс записи"
                    } else {
                        "изолированный просмотр"
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{access} · проект {project} · {source}"
                        ))
                        .color(theme::MUTED),
                    );
                });
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::TOP),
                    |ui| {
                        let refresh = ui.add_enabled(
                            http_available,
                            egui::Button::new("диагностика · обновить"),
                        );
                        let refresh = if http_available {
                            refresh
                        } else {
                            refresh.on_disabled_hover_text("Для диагностики требуется HTTP-шлюз")
                        };
                        if refresh.clicked() {
                            action = HomeAction::Refresh;
                        }
                        if loading {
                            ui.spinner();
                        }
                    },
                );
            });
            ui.add_space(14.0);

            if selected_project.is_none() {
                callout(
                    ui,
                    theme::WARN,
                    if http_available {
                        "Выберите проект"
                    } else {
                        "Источник только для чтения"
                    },
                    if http_available {
                        "Глобальное состояние шлюза уже видно, а инвентарь, комнаты и исходные каталоги требуют выбора проекта."
                    } else if wiki_available {
                        "Граф и вики доступны для просмотра; для корпуса, поиска, журнала и диагностики среды выполнения требуется HTTP-шлюз."
                    } else {
                        "Снимок графа доступен для просмотра; для документов и вики нужна БД или HTTP-шлюз, для остальных разделов — HTTP-шлюз."
                    },
                );
                ui.add_space(10.0);
            }
            if let Some(error) = project_error {
                callout(ui, theme::DANGER, "Пульт проекта недоступен", error);
                ui.add_space(8.0);
            }
            if let Some(error) = operations_error {
                let detail = if operations.is_some() {
                    format!("Показан последний успешный снимок. {error}")
                } else {
                    error.to_string()
                };
                callout(ui, theme::DANGER, "Диагностика не обновлена", &detail);
                ui.add_space(8.0);
            } else if operations_stale && operations.is_some() {
                callout(
                    ui,
                    theme::WARN,
                    "Диагностика устарела",
                    "Ниже показан последний успешный снимок; свежая проверка gateway не завершилась успешно.",
                );
                ui.add_space(8.0);
            }

            let status = operations.map(|snapshot| &snapshot.status);
            let project_scoped = selected_project.is_some();
            let documents = if project_scoped {
                home.map(|value| value.documents)
            } else {
                status.map(|value| value.document_count)
            };
            let chunks = if project_scoped {
                home.map(|value| value.chunks)
            } else {
                status.map(|value| value.chunk_count)
            };
            let nodes = if project_scoped {
                home.map(|value| value.graph_nodes)
            } else {
                status.map(|value| value.node_count)
            };
            let edges = if project_scoped {
                home.map(|value| value.graph_edges)
            } else {
                status.map(|value| value.edge_count)
            };
            let wiki = if project_scoped {
                home.map(|value| value.wiki_documents)
            } else {
                status.map(|value| value.wiki_count)
            };
            // The gateway exposes compilation debt and index coverage globally.
            // Showing them as project metrics would be precise-looking but false.
            let debt = if project_scoped {
                None
            } else {
                status.map(|value| value.uncompiled_raw_count)
            };
            let coverage = if project_scoped {
                None
            } else {
                status.map(|value| value.index_coverage.clamp(0.0, 1.0))
            };

            ui.columns(5, |columns| {
                metric_card(
                    &mut columns[0],
                    "Документы",
                    count_opt(documents),
                    home.map(|value| format!("{} исходных файлов", compact_count(value.source_files)))
                        .unwrap_or_else(|| "индексированные записи".into()),
                    theme::L0,
                );
                metric_card(
                    &mut columns[1],
                    "Чанки",
                    count_opt(chunks),
                    match (chunks, documents) {
                        (Some(chunks), Some(documents)) if documents > 0 => {
                            format!("≈{:.1} / документ", chunks as f64 / documents as f64)
                        }
                        _ => "единицы поиска".into(),
                    },
                    theme::L1,
                );
                metric_card(
                    &mut columns[2],
                    "Граф",
                    match (nodes, edges) {
                        (Some(nodes), Some(edges)) => {
                            format!("{} / {}", compact_count(nodes), compact_count(edges))
                        }
                        _ => "—".into(),
                    },
                    "узлы / рёбра".into(),
                    theme::L2,
                );
                metric_card(
                    &mut columns[3],
                    "Индекс вики",
                    coverage
                        .map(|value| format!("{:.1}%", value * 100.0))
                        .unwrap_or_else(|| "—".into()),
                    wiki.map(|value| format!("{} страниц", compact_count(value)))
                        .unwrap_or_else(|| "покрытие неизвестно".into()),
                    theme::L3,
                );
                metric_card(
                    &mut columns[4],
                    "Долг компиляции",
                    count_opt(debt),
                    "сырые документы без покрытия вики".into(),
                    if debt.unwrap_or_default() > 0 {
                        theme::WARN
                    } else {
                        theme::L4
                    },
                );
            });

            ui.add_space(11.0);
            ui.columns(2, |columns| {
                theme::card().show(&mut columns[0], |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Слои хранилища");
                        ui.label(
                            egui::RichText::new("верхние цитируют нижние · сырой слой неизменяем")
                                .small()
                                .color(theme::FAINT),
                        );
                    });
                    ui.add_space(8.0);
                    let agent_count = activity
                        .iter()
                        .filter_map(|event| event.client.as_deref())
                        .collect::<std::collections::HashSet<_>>()
                        .len();
                    if layer_row(
                        ui,
                        "L4",
                        "Клиенты и операции",
                        format!("{agent_count} источников в текущем журнале"),
                        theme::L4,
                        http_available,
                        if http_available { "журнал →" } else { "нужен HTTP" },
                    ) {
                        action = HomeAction::OpenAgents;
                    }
                    if layer_row(
                        ui,
                        "L3",
                        "Скомпилированное знание",
                        wiki.map(|value| format!("{} страниц вики", compact_count(value)))
                            .unwrap_or_else(|| "страницы не загружены".into()),
                        theme::L3,
                        wiki_available,
                        if wiki_available {
                            coverage
                                .map(|value| if value >= 1.0 { "готово" } else { "неполно" })
                                .unwrap_or(if http_available {
                                    "нет метрики проекта"
                                } else {
                                    "только чтение"
                                })
                        } else {
                            "нет в снимке"
                        },
                    ) {
                        action = HomeAction::OpenWiki;
                    }
                    if layer_row(
                        ui,
                        "L2",
                        "Граф объектов",
                        match (nodes, edges) {
                            (Some(nodes), Some(edges)) => format!(
                                "{} узлов · {} рёбер",
                                compact_count(nodes),
                                compact_count(edges)
                            ),
                            _ => "топология не загружена".into(),
                        },
                        theme::L2,
                        true,
                        home.map(|value| {
                            if value.unresolved_links == 0 {
                                "целостен"
                            } else {
                                "есть заглушки"
                            }
                        })
                        .unwrap_or("—"),
                    ) {
                        action = HomeAction::OpenGraph;
                    }
                    if layer_row(
                        ui,
                        "L1",
                        "Извлечение",
                        chunks
                            .map(|value| format!(
                                "{} чанков · лексический / векторный / гибридный",
                                compact_count(value)
                            ))
                            .unwrap_or_else(|| "поиск не загружен".into()),
                        theme::L1,
                        http_available,
                        if http_available {
                            status
                                .map(|value| if value.ready_for_search { "готов" } else { "требует внимания" })
                                .unwrap_or("—")
                        } else {
                            "нужен HTTP"
                        },
                    ) {
                        action = HomeAction::OpenSearch;
                    }
                    if layer_row(
                        ui,
                        "L0",
                        "Сырой корпус",
                        home.map(|value| {
                            format!(
                                "{} сырых · {} комнат · {} каталогов",
                                compact_count(value.raw_documents),
                                value.rooms,
                                value.source_roots.len()
                            )
                        })
                        .unwrap_or_else(|| {
                            status
                                .map(|value| format!("{} сырых", compact_count(value.raw_count)))
                                .unwrap_or_else(|| "корпус не загружен".into())
                        }),
                        theme::L0,
                        http_available,
                        if http_available { "неизменяемый" } else { "нужен HTTP" },
                    ) {
                        action = HomeAction::OpenLibrary;
                    }
                });

                columns[1].vertical(|ui| {
                    theme::card().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong("Диагностика");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !http_available {
                                        ui.label(
                                            egui::RichText::new("только чтение")
                                                .monospace()
                                                .color(theme::ACCENT),
                                        );
                                    } else if operations_stale && operations.is_some() {
                                        ui.label(
                                            egui::RichText::new("устарело")
                                                .monospace()
                                                .color(theme::WARN),
                                        );
                                    } else if let Some(snapshot) = operations {
                                        ui.label(
                                            egui::RichText::new(if snapshot.doctor.ok {
                                                "исправно"
                                            } else {
                                                "требует внимания"
                                            })
                                            .monospace()
                                            .color(if snapshot.doctor.ok {
                                                theme::OK
                                            } else {
                                                theme::DANGER
                                            }),
                                        );
                                    } else {
                                        ui.weak("проверка…");
                                    }
                                },
                            );
                        });
                        ui.add_space(8.0);
                        if !http_available {
                            theme::inset().show(ui, |ui| {
                                ui.strong("Диагностика среды выполнения недоступна");
                                ui.label(
                                    egui::RichText::new(
                                        "Режим только для чтения показывает граф и доступные документы. Состояние схемы, индексов и WAL требует подключения к HTTP-шлюзу.",
                                    )
                                    .color(theme::MUTED),
                                );
                            });
                        } else if let Some(snapshot) = operations {
                            let doctor = &snapshot.doctor;
                            doctor_row(
                                ui,
                                doctor.schema_ok,
                                format!(
                                    "версия схемы {} = {}",
                                    doctor.schema_version, doctor.expected_schema_version
                                ),
                            );
                            doctor_row(
                                ui,
                                doctor.fts_ready,
                                if doctor.fts_ready {
                                    "FTS готов · записи сразу видны при чтении".into()
                                } else {
                                    "FTS не готов".into()
                                },
                            );
                            doctor_row(
                                ui,
                                doctor.embed_ok,
                                if doctor.embed_ok {
                                    "Манифест эмбеддингов совпадает".into()
                                } else {
                                    "Расхождение манифеста эмбеддингов".into()
                                },
                            );
                            doctor_row(
                                ui,
                                doctor.relational_integrity_ok,
                                format!(
                                    "{} чанков без документа · {} рёбер без узла",
                                    doctor.orphan_chunks, doctor.orphan_edges
                                ),
                            );
                            doctor_row(
                                ui,
                                !doctor.wal_too_large,
                                format!(
                                    "WAL {} из {} MiB",
                                    doctor.wal_bytes / 1_048_576,
                                    doctor.wal_warn_bytes / 1_048_576
                                ),
                            );
                            doctor_row(
                                ui,
                                doctor.documents_without_chunks == 0,
                                format!(
                                    "{} документов без чанков",
                                    doctor.documents_without_chunks
                                ),
                            );
                        } else {
                            ui.weak("Диагностика ещё не загружена.");
                        }
                    });
                    ui.add_space(11.0);
                    theme::card().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong("Последняя активность");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let journal = ui.add_enabled(
                                        http_available,
                                        egui::Button::new("весь журнал →").small(),
                                    );
                                    let journal = if http_available {
                                        journal
                                    } else {
                                        journal.on_disabled_hover_text(
                                            "Для журнала операций требуется HTTP-шлюз",
                                        )
                                    };
                                    if journal.clicked() {
                                        action = HomeAction::OpenAgents;
                                    }
                                },
                            );
                        });
                        ui.add_space(7.0);
                        if activity.is_empty() {
                            ui.weak(if http_available {
                                "Операций пока нет."
                            } else {
                                "Журнал недоступен в режиме только для чтения."
                            });
                        }
                        for event in activity.iter().rev().take(6) {
                            ui.horizontal_wrapped(|ui| {
                                let (mark, color) = match event.status {
                                    Some(status) if status >= 400 => ("!", theme::DANGER),
                                    Some(_) => ("●", theme::OK),
                                    None => ("·", theme::FAINT),
                                };
                                ui.label(
                                    egui::RichText::new(mark).color(color),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        event.client.as_deref().unwrap_or("система"),
                                    )
                                    .strong(),
                                );
                                ui.label(egui::RichText::new(&event.action).color(theme::MUTED));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(short_time(&event.at))
                                                .monospace()
                                                .small()
                                                .color(theme::FAINT),
                                        );
                                    },
                                );
                            });
                        }
                    });
                });
            });

            if let Some(home) = home {
                ui.add_space(11.0);
                theme::card().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Исходные каталоги · свежесть");
                        if let Some(updated) = home.last_updated_at.as_deref() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "последнее изменение {}",
                                    readable_timestamp(updated)
                                ))
                                .small()
                                .color(theme::FAINT),
                            );
                        }
                    });
                    ui.add_space(7.0);
                    if home.source_roots.is_empty() {
                        ui.weak("Для проекта исходные каталоги не зарегистрированы.");
                    } else {
                        egui::Grid::new("console_source_roots")
                            .num_columns(3)
                            .striped(true)
                            .spacing([16.0, 7.0])
                            .show(ui, |ui| {
                                ui.strong("Каталог");
                                ui.strong("Файлы");
                                ui.strong("Последняя синхронизация");
                                ui.end_row();
                                for root in home.source_roots.iter().take(6) {
                                    ui.label(&root.canonical_root);
                                    ui.monospace(compact_count(root.file_count));
                                    ui.weak(
                                        root.last_synced
                                            .as_deref()
                                            .or(root.last_seen.as_deref())
                                            .map(readable_timestamp)
                                            .unwrap_or_else(|| "ещё не синхронизирован".into()),
                                    );
                                    ui.end_row();
                                }
                            });
                    }
                });
            }
            ui.add_space(22.0);
        });
    action
}

fn metric_card(
    ui: &mut egui::Ui,
    label: &str,
    value: String,
    detail: String,
    color: egui::Color32,
) {
    theme::card()
        .stroke(egui::Stroke::new(1.0, theme::rgba(color, 70)))
        .show(ui, |ui| {
            ui.set_min_height(76.0);
            ui.label(
                egui::RichText::new(label.to_uppercase())
                    .small()
                    .strong()
                    .color(theme::MUTED),
            );
            ui.label(egui::RichText::new(value).size(22.0).strong().color(color));
            ui.label(egui::RichText::new(detail).small().color(theme::FAINT));
        });
}

fn layer_row(
    ui: &mut egui::Ui,
    layer: &str,
    title: &str,
    detail: String,
    color: egui::Color32,
    enabled: bool,
    status: &str,
) -> bool {
    let response = theme::inset()
        .fill(theme::rgba(color, 12))
        .stroke(egui::Stroke::new(1.0, theme::rgba(color, 58)))
        .show(ui, |ui| {
            ui.set_min_height(36.0);
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(theme::rgba(color, 32))
                    .stroke(egui::Stroke::new(1.0, theme::rgba(color, 100)))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(7, 5))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(layer).monospace().strong().color(color));
                    });
                ui.vertical(|ui| {
                    ui.strong(title);
                    ui.label(egui::RichText::new(detail).small().color(theme::FAINT));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("›").color(theme::FAINT));
                    ui.label(egui::RichText::new(status).monospace().small().color(color));
                });
            });
        });
    if !enabled {
        response
            .response
            .on_hover_text("Раздел доступен после подключения к HTTP-шлюзу");
        return false;
    }
    let clickable = ui.interact(
        response.response.rect,
        response.response.id.with(layer),
        egui::Sense::click(),
    );
    if clickable.hovered() {
        ui.painter().rect_stroke(
            clickable.rect,
            8.0,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    clickable.clicked()
}

fn doctor_row(ui: &mut egui::Ui, ok: bool, label: String) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(if ok { "✓" } else { "!" })
                .monospace()
                .color(if ok { theme::OK } else { theme::DANGER }),
        );
        ui.label(label);
    });
}

fn callout(ui: &mut egui::Ui, color: egui::Color32, title: &str, detail: &str) {
    theme::inset()
        .fill(theme::rgba(color, 14))
        .stroke(egui::Stroke::new(1.0, theme::rgba(color, 90)))
        .show(ui, |ui| {
            ui.strong(title);
            ui.label(detail);
        });
}

fn count_opt(value: Option<u64>) -> String {
    value.map(compact_count).unwrap_or_else(|| "—".into())
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

fn readable_timestamp(value: &str) -> String {
    value.strip_suffix('Z').unwrap_or(value).replace('T', " ")
}

fn short_time(value: &str) -> String {
    value
        .split_once('T')
        .map(|(_, time)| time.trim_end_matches('Z').chars().take(8).collect())
        .unwrap_or_else(|| value.chars().take(8).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_count_keeps_small_values_precise() {
        assert_eq!(compact_count(9999), "9999");
        assert_eq!(compact_count(12_450), "12.4K");
        assert_eq!(compact_count(2_100_000), "2.1M");
    }

    #[test]
    fn timestamp_is_readable_without_timezone_noise() {
        assert_eq!(
            readable_timestamp("2026-09-02T12:00:00Z"),
            "2026-09-02 12:00:00"
        );
        assert_eq!(short_time("2026-09-02T12:34:56Z"), "12:34:56");
    }
}
