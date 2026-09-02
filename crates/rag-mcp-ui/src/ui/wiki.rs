//! Obsidian/Notion-style wiki browser: page list + article body with `[[wikilinks]]`.
//! Supports read view and an in-app editor (Save via HTTP PUT or exclusive `--db`).

use std::collections::{BTreeMap, HashSet};

use egui::{RichText, Sense, Ui};

use crate::load::{BacklinkItem, DocumentBody, WikiPageMeta};

/// Persistent focus id of the sidebar filter field (Ctrl+F targets it).
pub fn wiki_filter_id() -> egui::Id {
    egui::Id::new("wiki_filter_edit")
}

/// Mutable buffers while the article pane is in edit mode.
#[derive(Debug, Clone, Default)]
pub struct WikiEditBuffers {
    pub title: String,
    pub content: String,
    /// Revision loaded when edit started (for CAS If-Match).
    pub base_revision: Option<i64>,
    pub base_etag: Option<String>,
    pub dirty: bool,
}

impl WikiEditBuffers {
    /// Seed edit buffers from an open article (CAS base revision/etag preserved).
    pub fn from_article(art: &DocumentBody) -> Self {
        Self {
            title: art.title.clone(),
            content: art.content.clone(),
            base_revision: art.revision,
            base_etag: art.etag.clone(),
            dirty: false,
        }
    }
}

/// Toolbar actions from the edit view only (no navigation).
#[derive(Debug, Clone, Default)]
pub struct WikiEditAction {
    pub save: bool,
    pub cancel: bool,
    /// Discard edits and refetch the current revision (CAS conflict recovery).
    pub reload: bool,
}

/// Navigation / mode actions from the read view only (no save/cancel).
#[derive(Debug, Clone, Default)]
pub struct WikiNavAction {
    /// User clicked a `[[wikilink]]` target (label or slug text).
    pub open_link: Option<String>,
    /// Prefer opening a backlink by document id when set.
    pub open_id: Option<String>,
    pub start_edit: bool,
    /// Retry the last failed load (page body or catalog).
    pub retry: bool,
}

/// Read-pane dependencies grouped separately from navigation results.
pub struct WikiReadContext<'a> {
    pub known_titles: &'a HashSet<String>,
    pub known_slugs: &'a HashSet<String>,
    pub can_write: bool,
    pub salt_prefix: &'a str,
    pub loading: bool,
}

/// Left sidebar: searchable list of wiki pages, grouped by `category`.
pub fn draw_wiki_sidebar(
    ui: &mut Ui,
    pages: &[WikiPageMeta],
    filter: &mut String,
    selected_id: Option<&str>,
    show_summaries: &mut bool,
) -> Option<String> {
    let mut clicked: Option<String> = None;

    ui.horizontal(|ui| {
        ui.heading("Library");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(format!("{} pages", pages.len()));
        });
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(filter)
                .id(wiki_filter_id())
                .desired_width(f32::INFINITY)
                .hint_text("Search pages…  (Ctrl+F)"),
        );
        if ui
            .add_enabled(!filter.is_empty(), egui::Button::new("✕"))
            .on_hover_text("Clear filter")
            .clicked()
        {
            filter.clear();
        }
    });
    ui.checkbox(show_summaries, "Show summaries")
        .on_hover_text("Show a short description under each page");
    ui.separator();

    let q = filter.trim().to_lowercase();
    let matches_filter = |p: &WikiPageMeta| {
        if q.is_empty() {
            return true;
        }
        let hay = format!(
            "{} {} {} {}",
            p.title,
            p.slug,
            p.summary.as_deref().unwrap_or(""),
            p.category.as_deref().unwrap_or("")
        )
        .to_lowercase();
        hay.contains(&q)
    };

    egui::ScrollArea::vertical()
        .id_salt("wiki_sidebar_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if pages.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.strong("No pages in this project");
                    ui.weak("Choose another project or create a wiki page.");
                });
                return;
            }
            if !q.is_empty() {
                // Filtered: flat list (grouping would hide matches under closed headers).
                let matches: Vec<_> = pages.iter().filter(|p| matches_filter(p)).collect();
                if matches.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.strong("Nothing found");
                        ui.weak("Try a shorter or different search.");
                    });
                }
                for p in matches {
                    draw_page_entry(ui, p, selected_id, *show_summaries, &mut clicked);
                }
                return;
            }
            // Grouped by category; uncategorized last.
            let mut groups: BTreeMap<String, Vec<&WikiPageMeta>> = BTreeMap::new();
            for p in pages {
                groups
                    .entry(p.category.clone().unwrap_or_default())
                    .or_default()
                    .push(p);
            }
            let uncategorized = groups.remove("");
            for (cat, group) in &groups {
                egui::CollapsingHeader::new(cat)
                    .id_salt(format!("wiki_cat_{cat}"))
                    .default_open(true)
                    .show(ui, |ui| {
                        for p in group {
                            draw_page_entry(ui, p, selected_id, *show_summaries, &mut clicked);
                        }
                    });
            }
            if let Some(group) = uncategorized {
                if groups.is_empty() {
                    // No categories at all: flat list without a header.
                    for p in group {
                        draw_page_entry(ui, p, selected_id, *show_summaries, &mut clicked);
                    }
                } else {
                    egui::CollapsingHeader::new("Uncategorized")
                        .id_salt("wiki_cat_uncategorized")
                        .default_open(true)
                        .show(ui, |ui| {
                            for p in group {
                                draw_page_entry(ui, p, selected_id, *show_summaries, &mut clicked);
                            }
                        });
                }
            }
        });

    clicked
}

fn draw_page_entry(
    ui: &mut Ui,
    p: &WikiPageMeta,
    selected_id: Option<&str>,
    show_summary: bool,
    clicked: &mut Option<String>,
) {
    let selected = selected_id == Some(p.id.as_str());
    let label = if p.title.is_empty() {
        p.slug.as_str()
    } else {
        p.title.as_str()
    };
    if ui.selectable_label(selected, label).clicked() {
        *clicked = Some(p.id.clone());
    }
    if show_summary {
        if let Some(sum) = p.summary.as_deref() {
            if !sum.is_empty() {
                ui.weak(truncate(sum, 72));
            }
        }
    }
}

/// Edit mode only: title/content fields + Save/Cancel. No render/link/backlink params.
///
/// `saving` freezes the submitted buffers, disables Save/Cancel and shows a
/// spinner while the non-cancellable worker write is in flight.
pub fn draw_wiki_edit_view(
    ui: &mut Ui,
    page: &DocumentBody,
    error: Option<&str>,
    edit: &mut WikiEditBuffers,
    can_write: bool,
    saving: bool,
) -> WikiEditAction {
    let mut action = WikiEditAction::default();

    if let Some(err) = error {
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::from_rgb(220, 100, 100), err);
            // CAS conflict (HTTP 409 / revision mismatch): offer a way out.
            let conflict = err.contains("conflict") || err.contains("409");
            if conflict
                && ui
                    .add_enabled(!saving, egui::Button::new("Reload page"))
                    .on_hover_text("Discard edits and refetch the current revision")
                    .clicked()
            {
                action.reload = true;
            }
        });
        ui.separator();
    }

    ui.horizontal(|ui| {
        ui.heading("Edit");
        if edit.dirty {
            ui.colored_label(egui::Color32::from_rgb(220, 160, 60), "unsaved");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(can_write && !saving, egui::Button::new("Save"))
                .on_hover_text(if saving {
                    "Save in flight on the worker…"
                } else if can_write {
                    "Write via HTTP PUT /v1/wiki or exclusive --db"
                } else {
                    "Read-only source (snapshot has no write path)"
                })
                .clicked()
            {
                action.save = true;
            }
            if saving {
                ui.spinner();
            }
            if ui
                .add_enabled(can_cancel_edit(saving), egui::Button::new("Cancel"))
                .on_disabled_hover_text("Wait for the save result")
                .clicked()
            {
                action.cancel = true;
            }
            if let Some(r) = edit.base_revision {
                ui.weak(format!("base r{r}"));
            }
        });
    });
    ui.weak(format!("{} · {}", page.layer, page.uri));
    ui.separator();

    ui.label("Title");
    let title_resp = ui.add_enabled(
        !saving,
        egui::TextEdit::singleline(&mut edit.title)
            .desired_width(f32::INFINITY)
            .hint_text("Page title"),
    );
    if title_resp.changed() {
        edit.dirty = true;
    }
    ui.add_space(6.0);
    ui.label("Content");
    egui::ScrollArea::vertical()
        .id_salt("wiki_edit_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let content_resp = ui
                .add_enabled_ui(!saving, |ui| {
                    ui.add_sized(
                        [
                            ui.available_width(),
                            (ui.available_height() - 8.0).max(200.0),
                        ],
                        egui::TextEdit::multiline(&mut edit.content)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Markdown body…"),
                    )
                })
                .inner;
            if content_resp.changed() {
                edit.dirty = true;
            }
        });

    action
}

pub(crate) const fn can_cancel_edit(saving: bool) -> bool {
    !saving
}

/// Read mode: rendered markdown, Edit button, backlinks. No edit buffers.
///
/// `salt_prefix` ("a" / "b") + page id key the article scroll area, so each
/// pane keeps its own offset and the offset resets when the page changes.
pub fn draw_wiki_read_view(
    ui: &mut Ui,
    page: Option<&DocumentBody>,
    error: Option<&str>,
    context: WikiReadContext<'_>,
) -> WikiNavAction {
    let mut action = WikiNavAction::default();

    if let Some(err) = error {
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::from_rgb(220, 100, 100), err);
            if ui.button("Retry").clicked() {
                action.retry = true;
            }
        });
        ui.separator();
    }

    let Some(page) = page else {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            if context.loading {
                ui.spinner();
                ui.label("Loading page…");
            } else {
                ui.heading("Select a page");
                ui.weak("Pick a wiki note in the left sidebar.");
                ui.weak("Blue [[links]] resolve; grey = missing page.");
            }
        });
        return action;
    };

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&page.title).size(28.0).strong());
        if context.loading {
            // Next page is on the wire; this one stays visible until it lands.
            ui.spinner();
        }
    });
    ui.horizontal(|ui| {
        ui.weak(format!(
            "{} · revision {}",
            page.layer,
            page.revision.unwrap_or(1)
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(context.can_write, egui::Button::new("Edit"))
                .on_hover_text(if context.can_write {
                    "Edit this page (Save uses HTTP PUT or --db)"
                } else {
                    "Editing requires --http or --db"
                })
                .clicked()
            {
                action.start_edit = true;
            }
        });
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt(format!(
            "wiki_article_scroll:{}:{}",
            context.salt_prefix, page.id
        ))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let padding = ((ui.available_width() - 780.0) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(padding);
                ui.vertical(|ui| {
                    ui.set_max_width(780.0);
                    action.open_link = render_wiki_markdown(
                        ui,
                        &page.content,
                        context.known_titles,
                        context.known_slugs,
                    );
                    ui.add_space(24.0);
                });
            });
        });

    action
}

/// Secondary information panel: page identity and incoming links stay out of
/// the reading flow and can be hidden independently by the shell.
pub fn draw_wiki_info_panel(
    ui: &mut Ui,
    page: Option<&DocumentBody>,
    backlinks: &[BacklinkItem],
) -> WikiNavAction {
    let mut action = WikiNavAction::default();
    ui.heading("Page info");
    ui.separator();
    let Some(page) = page else {
        ui.weak("Select a page to see its details.");
        return action;
    };

    ui.weak("Location");
    ui.monospace(&page.uri);
    ui.add_space(10.0);
    ui.weak("Type");
    ui.label(format!("{} · {}", page.layer, page.kind));
    if let Some(revision) = page.revision {
        ui.label(format!("Revision {revision}"));
    }

    ui.add_space(18.0);
    ui.strong(format!("Backlinks · {}", backlinks.len()));
    ui.separator();
    if backlinks.is_empty() {
        ui.weak("No incoming links.");
    } else {
        for backlink in backlinks {
            let response = ui.add(
                egui::Label::new(
                    RichText::new(&backlink.label).color(egui::Color32::from_rgb(100, 160, 255)),
                )
                .sense(Sense::click()),
            );
            if response.clicked() {
                if backlink.id.is_empty() {
                    action.open_link = Some(backlink.label.clone());
                } else {
                    action.open_id = Some(backlink.id.clone());
                }
            }
        }
    }
    action
}

/// Slug segment from a `wiki://…` URI (or raw path if no prefix).
pub fn slug_from_wiki_uri(uri: &str) -> String {
    uri.strip_prefix("wiki://")
        .unwrap_or(uri)
        .trim()
        .to_string()
}

/// First non-empty line of content, truncated for catalog summary.
pub fn content_summary_line(content: &str) -> Option<String> {
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(240).collect::<String>())
}

/// Markdown-ish: headings, lists, fences, **bold**, `code`, [[links]], #tags.
fn render_wiki_markdown(
    ui: &mut Ui,
    content: &str,
    known_titles: &HashSet<String>,
    known_slugs: &HashSet<String>,
) -> Option<String> {
    let mut open_link: Option<String> = None;
    let mut in_code = false;
    let mut code_buf = String::new();

    for line in content.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("```") {
            if in_code {
                ui.group(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&code_buf).monospace())
                            .wrap()
                            .selectable(true),
                    );
                });
                code_buf.clear();
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }
        if in_code {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }

        if trimmed.is_empty() {
            ui.add_space(6.0);
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("### ") {
            ui.add_space(8.0);
            ui.label(RichText::new(rest).strong().size(16.0));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            ui.add_space(10.0);
            ui.label(RichText::new(rest).strong().size(18.0));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            ui.add_space(12.0);
            ui.label(RichText::new(rest).strong().size(22.0));
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("> ") {
            ui.horizontal(|ui| {
                ui.weak("│");
                if let Some(link) = draw_inline_with_wikilinks(ui, rest, known_titles, known_slugs)
                {
                    open_link = Some(link);
                }
            });
            continue;
        }

        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "));
        let numbered = if bullet.is_none() {
            let mut chars = trimmed.chars();
            if chars.next().is_some_and(|c| c.is_ascii_digit()) {
                if let Some(dot) = trimmed.find(". ") {
                    let (n, rest) = trimmed.split_at(dot);
                    if n.chars().all(|c| c.is_ascii_digit()) {
                        Some((n, &rest[2..]))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        ui.horizontal_wrapped(|ui| {
            if bullet.is_some() {
                ui.label("•");
            } else if let Some((n, _)) = numbered {
                ui.label(format!("{n}."));
            }
            let text = bullet.or(numbered.map(|(_, r)| r)).unwrap_or(trimmed);
            if let Some(link) = draw_inline_with_wikilinks(ui, text, known_titles, known_slugs) {
                open_link = Some(link);
            }
        });
    }

    if in_code && !code_buf.is_empty() {
        ui.group(|ui| {
            ui.add(
                egui::Label::new(RichText::new(&code_buf).monospace())
                    .wrap()
                    .selectable(true),
            );
        });
    }

    open_link
}

fn link_resolved(key: &str, titles: &HashSet<String>, slugs: &HashSet<String>) -> bool {
    let k = key.trim();
    titles.contains(k)
        || slugs.contains(k)
        || titles.iter().any(|t| t.eq_ignore_ascii_case(k))
        || slugs.iter().any(|s| s.eq_ignore_ascii_case(k))
}

fn draw_inline_with_wikilinks(
    ui: &mut Ui,
    text: &str,
    known_titles: &HashSet<String>,
    known_slugs: &HashSet<String>,
) -> Option<String> {
    let mut clicked: Option<String> = None;
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let before = &rest[..start];
        if !before.is_empty() {
            emit_rich_plain(ui, before);
        }
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("]]") {
            let target = after_open[..end].trim();
            let (link_key, display) = if let Some((a, b)) = target.split_once('|') {
                (a.trim(), b.trim())
            } else {
                (target, target)
            };
            let resolved = link_resolved(link_key, known_titles, known_slugs);
            let color = if resolved {
                egui::Color32::from_rgb(100, 160, 255)
            } else {
                egui::Color32::from_rgb(140, 140, 140)
            };
            let mut rich = RichText::new(display).color(color);
            if resolved {
                rich = rich.underline();
            }
            let mut resp = ui.add(egui::Label::new(rich).sense(Sense::click()));
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if !resolved {
                resp = resp.on_hover_text("Unresolved - no page with this title/slug");
            }
            if resp.clicked() && !link_key.is_empty() {
                clicked = Some(link_key.to_string());
            }
            rest = &after_open[end + 2..];
        } else {
            emit_rich_plain(ui, &rest[start..]);
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        emit_rich_plain(ui, rest);
    }
    clicked
}

/// Plain segments with **bold**, `code`, #tags.
fn emit_rich_plain(ui: &mut Ui, text: &str) {
    let mut rest = text;
    while !rest.is_empty() {
        // code `...`
        if let Some(c0) = rest.find('`') {
            if c0 > 0 {
                emit_bold_and_tags(ui, &rest[..c0]);
            }
            let after = &rest[c0 + 1..];
            if let Some(c1) = after.find('`') {
                ui.label(
                    RichText::new(&after[..c1])
                        .monospace()
                        .background_color(egui::Color32::from_rgb(40, 40, 48)),
                );
                rest = &after[c1 + 1..];
                continue;
            }
            ui.label("`");
            rest = after;
            continue;
        }
        emit_bold_and_tags(ui, rest);
        break;
    }
}

fn emit_bold_and_tags(ui: &mut Ui, text: &str) {
    let mut rest = text;
    while let Some(b0) = rest.find("**") {
        if b0 > 0 {
            emit_tags_only(ui, &rest[..b0]);
        }
        let after = &rest[b0 + 2..];
        if let Some(b1) = after.find("**") {
            ui.label(RichText::new(&after[..b1]).strong());
            rest = &after[b1 + 2..];
        } else {
            emit_tags_only(ui, &rest[b0..]);
            return;
        }
    }
    if !rest.is_empty() {
        emit_tags_only(ui, rest);
    }
}

fn emit_tags_only(ui: &mut Ui, text: &str) {
    let mut rest = text;
    while let Some(hash_at) = rest.find('#') {
        let before = &rest[..hash_at];
        if !before.is_empty() {
            ui.label(before);
        }
        let after = &rest[hash_at + 1..];
        let tag_end = after
            .find(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == ')' || c == ']')
            .unwrap_or(after.len());
        let tag = &after[..tag_end];
        if tag.is_empty() {
            ui.label("#");
            rest = after;
            continue;
        }
        ui.label(RichText::new(format!("#{tag}")).color(egui::Color32::from_rgb(180, 140, 220)));
        rest = &after[tag_end..];
    }
    if !rest.is_empty() {
        ui.label(rest);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::{can_cancel_edit, content_summary_line, slug_from_wiki_uri};

    #[test]
    fn cancel_is_disabled_while_wiki_save_is_in_flight() {
        assert!(!can_cancel_edit(true));
        assert!(can_cancel_edit(false));
    }

    #[test]
    fn slug_from_wiki_uri_strips_prefix() {
        assert_eq!(slug_from_wiki_uri("wiki://notes/foo"), "notes/foo");
        assert_eq!(slug_from_wiki_uri("notes/foo"), "notes/foo");
        assert_eq!(slug_from_wiki_uri("wiki://x  "), "x");
        // Leading space: same as prior inline path (strip only at start, then trim).
        assert_eq!(slug_from_wiki_uri("  wiki://x  "), "wiki://x");
    }

    #[test]
    fn content_summary_line_first_nonempty() {
        assert_eq!(
            content_summary_line("\n\nHello\nWorld"),
            Some("Hello".into())
        );
        assert_eq!(content_summary_line("   \n  "), None);
    }
}
