//! Empty / error banners with one primary action hint.
//!
//! When a graph source is loaded but no seed is set, the canvas must not paint
//! a global hairball: prompt for seed label/id and show loaded topology stats.

use egui::Ui;
use rag_mcp::GraphView;
use std::collections::BTreeMap;

use super::theme;

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
            "Подключите источник",
            "Запустите с --http URL, --snapshot PATH или --db PATH",
        ),
        EmptyKind::EmptyGraph => (
            "Граф пуст",
            "Проиндексируйте документы или откройте другой источник",
        ),
        EmptyKind::MissingSeed => (
            "Выберите стартовый узел",
            "Введите метку или id в панели графа и нажмите Enter. При запуске можно передать --seed.",
        ),
        EmptyKind::SeedNotFound => (
            "Узел не найден",
            "Введите node id, отображаемую метку или document_id",
        ),
        EmptyKind::LoadError => ("Источник не загружен", "Повторите или откройте снимок"),
        EmptyKind::FiltersEmpty => (
            "Фильтры скрыли все узлы",
            "Включите tags/stubs или сбросьте фильтры и перестройте граф",
        ),
    };

    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.18);
        ui.set_max_width(620.0);
        theme::card().show(ui, |ui| {
            ui.label(
                egui::RichText::new("СОСТОЯНИЕ ГРАФА")
                    .monospace()
                    .small()
                    .color(theme::L2),
            );
            ui.heading(title);
            if let Some(d) = detail {
                ui.add_space(6.0);
                ui.colored_label(theme::DANGER, d);
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new(hint).color(theme::MUTED));

            // Explicit seed prompt on the two states where a seed is the blocker.
            if matches!(kind, EmptyKind::MissingSeed | EmptyKind::SeedNotFound) {
                ui.add_space(12.0);
                theme::inset().show(ui, |ui| {
                    ui.strong("Для seed можно указать");
                    ui.label("node id · отображаемая метка · document_id");
                    ui.weak(
                        "Соседи глубины 1 (максимум 100 000) загружаются только после разрешения seed.",
                    );
                });
            }

            if let Some(s) = stats {
                ui.add_space(16.0);
                draw_loaded_stats(ui, s);
            }
        });
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
        ui.add_space(ui.available_height() * 0.12);
        ui.set_max_width(620.0);
        theme::card().show(ui, |ui| {
            ui.label(
                egui::RichText::new("RAG-КОНСОЛЬ · НАТИВНЫЙ ИНТЕРФЕЙС")
                    .monospace()
                    .small()
                    .color(theme::L3),
            );
            ui.heading(if error.is_some() {
                "Не удалось открыть источник"
            } else {
                "Подключите базу знаний"
            });
            ui.label(
                egui::RichText::new(
                    "Полный продуктовый режим работает через единый HTTP-сервис записи.",
                )
                .color(theme::MUTED),
            );
            if let Some(err) = error {
                ui.add_space(8.0);
                theme::inset()
                    .fill(theme::rgba(theme::DANGER, 14))
                    .show(ui, |ui| {
                        ui.colored_label(theme::DANGER, err);
                    });
            }

            ui.add_space(14.0);
            ui.strong("HTTP-шлюз");
            ui.horizontal(|ui| {
                let field_width = (ui.available_width() - 130.0).max(240.0);
                let response = ui.add(
                    egui::TextEdit::singleline(connect_url)
                        .desired_width(field_width)
                        .hint_text("http://127.0.0.1:7432"),
                );
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui.button("Подключить").clicked() || enter {
                    action = NoSourceAction::Connect;
                }
            });
            ui.label(
                egui::RichText::new("Переключение происходит без перезапуска приложения.")
                    .small()
                    .color(theme::FAINT),
            );

            ui.add_space(14.0);
            theme::inset().show(ui, |ui| {
                mode_row(
                    ui,
                    "ОНЛАЙН",
                    "rag-mcp-ui --http http://127.0.0.1:7432",
                    "проекты · корпус · поиск · вики · операции",
                    theme::OK,
                );
                ui.separator();
                mode_row(
                    ui,
                    "СНИМОК",
                    "rag-mcp-ui --snapshot graph.json",
                    "безопасный просмотр графа только для чтения",
                    theme::L2,
                );
                ui.separator();
                mode_row(
                    ui,
                    "DUCKDB",
                    "rag-mcp-ui --db rag.duckdb",
                    "эксклюзивное открытие только для чтения; MCP должен быть остановлен",
                    theme::L0,
                );
            });

            if can_retry {
                ui.add_space(10.0);
                if ui
                    .button("Повторить исходное подключение")
                    .on_hover_text("Повторить источник, переданный в командной строке")
                    .clicked()
                {
                    action = NoSourceAction::Retry;
                }
            }
        });
    });
    action
}

fn mode_row(ui: &mut Ui, badge: &str, command: &str, detail: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [76.0, 24.0],
            egui::Label::new(egui::RichText::new(badge).monospace().small().color(color)),
        );
        ui.vertical(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(command).monospace())
                    .selectable(true)
                    .wrap(),
            );
            ui.label(egui::RichText::new(detail).small().color(theme::FAINT));
        });
    });
}

fn draw_loaded_stats(ui: &mut Ui, s: &EmptyGraphStats) {
    theme::inset().show(ui, |ui| {
        ui.set_max_width(480.0);
        ui.strong("Загруженный граф");
        ui.horizontal(|ui| {
            ui.label(format!("узлы={}", s.nodes));
            ui.separator();
            ui.label(format!("рёбра={}", s.edges));
            if s.truncated {
                ui.colored_label(theme::WARN, "ограничено");
            }
        });
        if let Some(raw) = s.raw_nodes {
            ui.weak(format!(
                "в источнике {raw} узлов; после жёсткого лимита показано {}",
                s.nodes
            ));
        }
        if !s.nodes_by_kind.is_empty() {
            ui.add_space(4.0);
            ui.label("Узлы по kind");
            ui.horizontal_wrapped(|ui| {
                for (kind, n) in &s.nodes_by_kind {
                    ui.monospace(format!("{kind}:{n}"));
                }
            });
        }
        if !s.edges_by_rel_type.is_empty() {
            ui.add_space(2.0);
            ui.label("Рёбра по rel_type");
            ui.horizontal_wrapped(|ui| {
                for (rel, n) in &s.edges_by_rel_type {
                    ui.monospace(format!("{rel}:{n}"));
                }
            });
        }
        ui.add_space(4.0);
        ui.weak(
            "Отрисовка начинается после seed: только локальные соседи (RadialLocal), без глобального графа.",
        );
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
