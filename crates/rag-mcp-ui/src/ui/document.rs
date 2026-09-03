//! Shared readable document body for Library and Graph inspectors.

use super::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReaderMode {
    Markdown,
    Code,
    Plain,
}

pub fn draw_document_reader(ui: &mut egui::Ui, content: &str, kind: &str, uri: &str) {
    let mode = reader_mode(content, kind, uri);
    let width = ui.available_width().min(760.0);
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| match mode {
            ReaderMode::Markdown => draw_markdown(ui, content),
            ReaderMode::Code => draw_code(ui, content),
            ReaderMode::Plain => draw_plain(ui, content),
        },
    );
}

fn reader_mode(content: &str, kind: &str, uri: &str) -> ReaderMode {
    let kind = kind.to_ascii_lowercase();
    let uri = uri.to_ascii_lowercase();
    if kind == "code"
        || [
            ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".cs", ".sql", ".c", ".cc", ".cpp",
            ".h", ".hpp", ".java", ".kt", ".swift", ".rb", ".php", ".sh", ".bash", ".zsh", ".fish",
            ".html", ".css", ".scss", ".vue", ".svelte", ".proto", ".json", ".yaml", ".yml",
            ".toml", ".xml", ".ini", ".cfg", ".conf",
        ]
        .iter()
        .any(|extension| uri.ends_with(extension))
    {
        ReaderMode::Code
    } else if kind == "wiki"
        || uri.ends_with(".md")
        || uri.starts_with("wiki://")
        || content.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("# ") || line.starts_with("## ") || line.starts_with("```")
        })
    {
        ReaderMode::Markdown
    } else {
        ReaderMode::Plain
    }
}

fn draw_plain(ui: &mut egui::Ui, content: &str) {
    for paragraph in content.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        ui.add(
            egui::Label::new(egui::RichText::new(paragraph).size(14.0))
                .wrap()
                .selectable(true),
        );
        ui.add_space(8.0);
    }
}

fn draw_code(ui: &mut egui::Ui, content: &str) {
    theme::inset().fill(theme::RAIL).show(ui, |ui| {
        egui::ScrollArea::horizontal()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(content)
                            .monospace()
                            .size(11.5)
                            .color(theme::TEXT),
                    )
                    .extend()
                    .selectable(true),
                );
            });
    });
}

fn draw_markdown(ui: &mut egui::Ui, content: &str) {
    let mut in_code = false;
    let mut code = String::new();
    for line in content.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("```") {
            if in_code {
                draw_code(ui, code.trim_end());
                code.clear();
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        if trimmed.is_empty() {
            ui.add_space(6.0);
        } else if let Some(text) = trimmed.strip_prefix("### ") {
            heading(ui, text, 16.0, 7.0);
        } else if let Some(text) = trimmed.strip_prefix("## ") {
            heading(ui, text, 18.0, 9.0);
        } else if let Some(text) = trimmed.strip_prefix("# ") {
            heading(ui, text, 22.0, 11.0);
        } else if let Some(text) = trimmed.strip_prefix("> ") {
            theme::inset()
                .fill(theme::rgba(theme::L3, 12))
                .stroke(egui::Stroke::new(1.0, theme::rgba(theme::L3, 70)))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).italics().color(theme::MUTED))
                            .wrap()
                            .selectable(true),
                    );
                });
        } else if let Some(text) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("•").color(theme::L3));
                ui.add(egui::Label::new(text).wrap().selectable(true));
            });
        } else {
            ui.add(
                egui::Label::new(egui::RichText::new(trimmed).size(14.0))
                    .wrap()
                    .selectable(true),
            );
        }
    }
    if in_code && !code.is_empty() {
        draw_code(ui, code.trim_end());
    }
}

fn heading(ui: &mut egui::Ui, text: &str, size: f32, top_space: f32) {
    ui.add_space(top_space);
    ui.add(
        egui::Label::new(egui::RichText::new(text).strong().size(size))
            .wrap()
            .selectable(true),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_uses_monospace_only_for_code_like_sources() {
        assert_eq!(
            reader_mode("fn main() {}", "code", "file:///main.rs"),
            ReaderMode::Code
        );
        assert_eq!(
            reader_mode("# Title", "wiki", "wiki://title"),
            ReaderMode::Markdown
        );
        assert_eq!(
            reader_mode("A plain note", "document", "file:///note.txt"),
            ReaderMode::Plain
        );
        assert_eq!(
            reader_mode(
                "service:\n  port: 7432",
                "document",
                "project://config.yaml"
            ),
            ReaderMode::Code
        );
    }
}
