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
    let available = ui.available_width();
    let width = available.min(820.0);
    ui.horizontal(|ui| {
        ui.add_space(((available - width) * 0.5).max(0.0));
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| match mode {
                ReaderMode::Markdown => draw_markdown(ui, content),
                ReaderMode::Code => draw_code(ui, content),
                ReaderMode::Plain => draw_plain(ui, content),
            },
        );
    });
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
    let lines: Vec<&str> = content.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_end();
        if let Some(language) = line.trim_start().strip_prefix("```") {
            let start = index + 1;
            index = start;
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                index += 1;
            }
            draw_code_block(ui, language.trim(), &lines[start..index].join("\n"));
            index += usize::from(index < lines.len());
            continue;
        }
        if line.trim().is_empty() {
            ui.add_space(7.0);
            index += 1;
            continue;
        }
        if is_rule(line) {
            ui.add_space(5.0);
            ui.separator();
            ui.add_space(5.0);
            index += 1;
            continue;
        }
        if index + 1 < lines.len() && is_table_separator(lines[index + 1]) && line.contains('|') {
            let start = index;
            index += 2;
            while index < lines.len()
                && lines[index].contains('|')
                && !lines[index].trim().is_empty()
            {
                index += 1;
            }
            draw_table(ui, &lines[start..index]);
            continue;
        }
        if let Some((level, text)) = markdown_heading(line) {
            let sizes = [27.0, 22.0, 18.0, 16.0, 15.0, 14.0];
            heading(
                ui,
                text,
                sizes[level - 1],
                if level == 1 { 16.0 } else { 10.0 },
            );
            if level <= 2 {
                ui.add_space(2.0);
                ui.separator();
            }
            index += 1;
            continue;
        }
        if line.trim_start().starts_with('>') {
            let start = index;
            while index < lines.len() && lines[index].trim_start().starts_with('>') {
                index += 1;
            }
            let quote = lines[start..index]
                .iter()
                .map(|line| line.trim_start().trim_start_matches('>').trim_start())
                .collect::<Vec<_>>()
                .join(" ");
            draw_quote(ui, &quote);
            continue;
        }
        if let Some((marker, text, indent)) = list_item(line) {
            draw_list_item(ui, marker, text, indent);
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < lines.len() && is_paragraph_continuation(lines[index]) {
            index += 1;
        }
        let paragraph = lines[start..index]
            .iter()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join(" ");
        ui.horizontal_wrapped(|ui| draw_inline(ui, &paragraph));
        ui.add_space(3.0);
    }
}

fn draw_code_block(ui: &mut egui::Ui, language: &str, code: &str) {
    theme::inset().fill(theme::RAIL).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(if language.is_empty() {
                    "CODE"
                } else {
                    language
                })
                .small()
                .strong()
                .color(theme::L1),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Копировать").clicked() {
                    ui.ctx().copy_text(code.to_string());
                }
            });
        });
        ui.separator();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(code).monospace().size(12.0))
                    .selectable(true)
                    .extend(),
            );
        });
    });
    ui.add_space(5.0);
}

fn draw_quote(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::rgba(theme::L3, 12))
        .stroke(egui::Stroke::new(1.0, theme::rgba(theme::L3, 85)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(13, 10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| draw_inline(ui, text));
        });
}

fn draw_list_item(ui: &mut egui::Ui, marker: &str, text: &str, indent: usize) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space((indent.min(4) as f32) * 16.0);
        let (marker, text) = match text.strip_prefix("[ ] ") {
            Some(text) => ("☐", text),
            None => match text
                .strip_prefix("[x] ")
                .or_else(|| text.strip_prefix("[X] "))
            {
                Some(text) => ("☑", text),
                None => (marker, text),
            },
        };
        ui.label(egui::RichText::new(marker).strong().color(theme::L3));
        draw_inline(ui, text);
    });
}

fn draw_table(ui: &mut egui::Ui, lines: &[&str]) {
    if lines.len() < 2 {
        return;
    }
    let rows: Vec<Vec<&str>> = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 1)
        .map(|(_, line)| table_cells(line))
        .collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    theme::inset().show(ui, |ui| {
        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::Grid::new(ui.next_auto_id())
                .num_columns(columns.max(1))
                .striped(true)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    for (row_index, row) in rows.iter().enumerate() {
                        for column in 0..columns {
                            let text = row.get(column).copied().unwrap_or("");
                            if row_index == 0 {
                                ui.strong(text);
                            } else {
                                ui.horizontal_wrapped(|ui| draw_inline(ui, text));
                            }
                        }
                        ui.end_row();
                    }
                });
        });
    });
}

fn draw_inline(ui: &mut egui::Ui, text: &str) {
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(code) = delimited(rest, "`", "`") {
            emit_plain(ui, code.before);
            ui.label(
                egui::RichText::new(code.inside)
                    .monospace()
                    .background_color(theme::RAIL)
                    .color(theme::L1),
            );
            rest = code.after;
        } else if let Some(bold) = delimited(rest, "**", "**") {
            emit_plain(ui, bold.before);
            ui.label(egui::RichText::new(bold.inside).strong());
            rest = bold.after;
        } else if let Some(link) = markdown_link(rest) {
            emit_plain(ui, link.before);
            ui.hyperlink_to(link.label, link.url);
            rest = link.after;
        } else {
            emit_plain(ui, rest);
            break;
        }
    }
}

fn emit_plain(ui: &mut egui::Ui, text: &str) {
    if !text.is_empty() {
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(14.5))
                .selectable(true)
                .wrap(),
        );
    }
}

struct InlineSpan<'a> {
    before: &'a str,
    inside: &'a str,
    after: &'a str,
}

fn delimited<'a>(text: &'a str, open: &str, close: &str) -> Option<InlineSpan<'a>> {
    let start = text.find(open)?;
    let inside_start = start + open.len();
    let end = text[inside_start..].find(close)? + inside_start;
    Some(InlineSpan {
        before: &text[..start],
        inside: &text[inside_start..end],
        after: &text[end + close.len()..],
    })
}

struct MarkdownLink<'a> {
    before: &'a str,
    label: &'a str,
    url: &'a str,
    after: &'a str,
}

fn markdown_link(text: &str) -> Option<MarkdownLink<'_>> {
    let start = text.find('[')?;
    let label_end = text[start + 1..].find("](")? + start + 1;
    let url_start = label_end + 2;
    let url_end = text[url_start..].find(')')? + url_start;
    Some(MarkdownLink {
        before: &text[..start],
        label: &text[start + 1..label_end],
        url: &text[url_start..url_end],
        after: &text[url_end + 1..],
    })
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|char| *char == '#').count();
    (1..=6).contains(&hashes).then(|| {
        trimmed
            .get(hashes..)?
            .strip_prefix(' ')
            .map(|text| (hashes, text))
    })?
}

fn list_item(line: &str) -> Option<(&str, &str, usize)> {
    let indent = line.len().saturating_sub(line.trim_start().len()) / 2;
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = trimmed.strip_prefix(marker) {
            return Some(("•", text, indent));
        }
    }
    let split = trimmed.find(". ")?;
    trimmed[..split]
        .chars()
        .all(|char| char.is_ascii_digit())
        .then(|| (&trimmed[..split + 1], &trimmed[split + 2..], indent))
}

fn table_cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

fn is_table_separator(line: &str) -> bool {
    let cells = table_cells(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim_matches(':');
            cell.len() >= 3 && cell.chars().all(|char| char == '-')
        })
}

fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|char| !char.is_whitespace()).collect();
    compact.len() >= 3
        && compact
            .chars()
            .all(|char| char == '-' || char == '*' || char == '_')
}

fn is_paragraph_continuation(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty()
        && !line.starts_with('#')
        && !line.starts_with('>')
        && !line.starts_with("```")
        && !is_rule(line)
        && list_item(line).is_none()
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

    #[test]
    fn markdown_blocks_recognize_reader_structures() {
        assert_eq!(markdown_heading("### Details"), Some((3, "Details")));
        assert_eq!(list_item("  - [x] Done"), Some(("•", "[x] Done", 1)));
        assert!(is_table_separator("| --- | :---: |"));
        assert!(is_rule("* * *"));
        assert!(!is_rule("ordinary text"));
    }

    #[test]
    fn markdown_link_keeps_surrounding_text() {
        let link = markdown_link("See [RAG](https://example.test/rag) now").expect("link");
        assert_eq!(link.before, "See ");
        assert_eq!(link.label, "RAG");
        assert_eq!(link.url, "https://example.test/rag");
        assert_eq!(link.after, " now");
    }
}
