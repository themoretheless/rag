//! Wiki browser: catalog, page open/history, edit/save, backlinks, link follow.

use super::*;

impl GraphApp {
    pub(super) fn ensure_wiki_loaded(&mut self) {
        if self.wiki_loaded || self.pending_catalog.is_some() {
            return;
        }
        self.reload_wiki_catalog();
    }

    pub(super) fn reload_wiki_catalog(&mut self) {
        self.wiki_error = None;
        let Some(source) = self.available_load_source() else {
            self.wiki_loaded = true;
            self.wiki_error = Some(if self.open.source.is_some() {
                "В snapshot-режиме нет каталога вики; используйте --http или --db".into()
            } else {
                "Источник данных не выбран".into()
            });
            return;
        };
        let seq = self.next_seq();
        self.pending_catalog = Some(seq);
        self.worker.send(WorkerCmd::LoadWikiCatalog {
            seq,
            source,
            project: nonempty(&self.filter_wing),
        });
    }

    /// Default page after the catalog lands: seed title match, else overview/first.
    pub(super) fn auto_open_initial_page(&mut self) {
        if self.wiki_selected_id.is_some() {
            return;
        }
        if !self.seed_input.trim().is_empty() {
            let q = self.seed_input.trim();
            if let Some(p) = self.wiki_pages.iter().find(|p| {
                p.title.eq_ignore_ascii_case(q) || p.slug.eq_ignore_ascii_case(q) || p.id == q
            }) {
                let id = p.id.clone();
                self.open_wiki_page_id(&id);
            }
        } else if let Some(p) = self
            .wiki_pages
            .iter()
            .find(|p| p.title.to_lowercase().contains("обзор") || p.slug.contains("overview"))
            .or_else(|| self.wiki_pages.first())
        {
            let id = p.id.clone();
            self.open_wiki_page_id(&id);
        }
    }

    pub(super) fn open_wiki_page_id(&mut self, id: &str) {
        if self.wiki_dual_pane && self.wiki_focus == WikiPane::B {
            // Re-clicking the page already open in B must not refetch body+backlinks.
            if self.wiki_selected_id_b.as_deref() == Some(id) && self.wiki_article_b.is_some() {
                return;
            }
            self.open_wiki_page_in_b(id);
            return;
        }
        if self.pending_save.is_some() {
            self.wiki_error = Some(
                "Идёт сохранение. Дождитесь результата перед открытием другой страницы.".into(),
            );
            return;
        }
        // Same for pane A.
        if self.wiki_selected_id.as_deref() == Some(id) && self.wiki_article.is_some() {
            return;
        }
        if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("Есть несохранённые правки: сохраните или отмените их".into());
            return;
        }
        if let Some(prev) = self.wiki_selected_id.clone() {
            if prev != id {
                self.wiki_history.push(prev);
                if self.wiki_history.len() > 64 {
                    self.wiki_history.remove(0);
                }
            }
        }
        self.open_wiki_page_id_no_history(id);
    }

    pub(super) fn open_wiki_page_id_no_history(&mut self, id: &str) {
        self.wiki_selected_id = Some(id.to_string());
        self.wiki_error = None;
        self.wiki_edit = None;
        self.wiki_save_note = None;
        let Some(source) = self.available_load_source() else {
            self.wiki_error = Some("Для статей вики требуется --http или --db".into());
            return;
        };
        let Some(meta) = self.wiki_pages.iter().find(|p| p.id == id).cloned() else {
            self.wiki_error = Some(format!("Страница {id} отсутствует в каталоге"));
            return;
        };
        // Keep the current article visible until the new body arrives.
        let seq = self.next_seq();
        self.pending_page_a = Some(seq);
        self.worker.send(WorkerCmd::OpenPage {
            seq,
            pane_b: false,
            push_history: false,
            meta: Some(meta),
            q: None,
            source,
        });
    }

    /// Load a page into the right (secondary) dual-pane column. No history / edit.
    pub(super) fn open_wiki_page_in_b(&mut self, id: &str) {
        self.wiki_selected_id_b = Some(id.to_string());
        self.wiki_error_b = None;
        let Some(source) = self.available_load_source() else {
            self.wiki_error_b = Some("Для статей вики требуется --http или --db".into());
            return;
        };
        let Some(meta) = self.wiki_pages.iter().find(|p| p.id == id).cloned() else {
            self.wiki_error_b = Some(format!("Страница {id} отсутствует в каталоге"));
            return;
        };
        let seq = self.next_seq();
        self.pending_page_b = Some(seq);
        self.worker.send(WorkerCmd::OpenPage {
            seq,
            pane_b: true,
            push_history: false,
            meta: Some(meta),
            q: None,
            source,
        });
    }

    pub(super) fn clear_wiki_pane_b(&mut self) {
        self.wiki_selected_id_b = None;
        self.wiki_article_b = None;
        self.wiki_error_b = None;
        self.pending_page_b = None;
    }

    pub(super) fn reset_wiki_for_project(&mut self) {
        self.worker.cancel_wiki_reads();
        self.pending_catalog = None;
        self.pending_page_a = None;
        self.pending_page_b = None;
        self.pending_backlinks = None;
        self.wiki_pages.clear();
        self.wiki_selected_id = None;
        self.wiki_article = None;
        self.wiki_backlinks.clear();
        self.wiki_backlinks_error = None;
        self.wiki_history.clear();
        self.wiki_error = None;
        self.wiki_edit = None;
        self.wiki_save_note = None;
        self.wiki_open_after_catalog = None;
        self.clear_wiki_pane_b();
        self.wiki_loaded = false;
        if self.mode == ViewMode::Wiki {
            self.reload_wiki_catalog();
        }
    }

    /// Apply a worker PageOpened result to the target pane.
    pub(super) fn apply_page_opened(
        &mut self,
        pane_b: bool,
        push_history: bool,
        q: Option<&str>,
        result: Result<DocumentBody, String>,
    ) {
        match result {
            Ok(body) => {
                // Unresolved-link fallback may open a page that is not in the
                // catalog yet; add a row so the sidebar stays consistent.
                if !self.wiki_pages.iter().any(|p| p.id == body.id) {
                    let slug = slug_from_wiki_uri(&body.uri);
                    self.wiki_pages.push(WikiPageMeta {
                        id: body.id.clone(),
                        uri: body.uri.clone(),
                        slug,
                        title: body.title.clone(),
                        kind: body.kind.clone(),
                        summary: None,
                        category: None,
                        revision: body.revision.unwrap_or(1),
                        etag: body.etag.clone(),
                        updated_at: body.updated_at.clone(),
                    });
                    sort_wiki_pages(&mut self.wiki_pages);
                }
                if pane_b {
                    self.wiki_selected_id_b = Some(body.id.clone());
                    self.wiki_article_b = Some(body);
                    self.wiki_error_b = None;
                } else {
                    let body_id = body.id.clone();
                    if push_history {
                        if let Some(prev) = self.wiki_selected_id.clone() {
                            if prev != body.id {
                                self.wiki_history.push(prev);
                                if self.wiki_history.len() > 64 {
                                    self.wiki_history.remove(0);
                                }
                            }
                        }
                    }
                    self.wiki_selected_id = Some(body.id.clone());
                    self.wiki_article = Some(body);
                    self.wiki_backlinks.clear();
                    self.wiki_backlinks_error = None;
                    self.wiki_error = None;
                    self.wiki_save_note = None;
                    self.refresh_backlinks(&body_id);
                }
            }
            Err(e) => {
                let msg = match q {
                    Some(q) => format!(
                        "Ссылка [[{q}]] не разрешена: страницы с таким названием/slug нет (создайте через write_wiki_page): {e}"
                    ),
                    None => e,
                };
                if pane_b {
                    self.wiki_article_b = None;
                    self.wiki_error_b = Some(msg);
                } else {
                    self.wiki_article = None;
                    self.wiki_backlinks.clear();
                    self.wiki_backlinks_error = None;
                    self.wiki_error = Some(msg);
                }
            }
        }
    }

    pub(super) fn wiki_can_write(&self) -> bool {
        self.activity_base().is_some()
    }

    pub(super) fn start_wiki_edit(&mut self) {
        let Some(art) = self.wiki_article.as_ref() else {
            return;
        };
        if !self.wiki_can_write() {
            self.wiki_error = Some(
                "Для редактирования нужен HTTP gateway; режим --db доступен только для чтения"
                    .into(),
            );
            return;
        }
        self.wiki_save_note = None;
        self.wiki_error = None;
        self.wiki_edit = Some(WikiEditBuffers::from_article(art));
    }

    pub(super) fn cancel_wiki_edit(&mut self) {
        if self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед закрытием редактора.".into());
            return;
        }
        self.wiki_edit = None;
        self.wiki_error = None;
    }

    /// Discard edits and refetch the current revision (409 CAS conflict path).
    pub(super) fn reload_wiki_page(&mut self) {
        if self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед обновлением страницы.".into());
            return;
        }
        let id = self
            .wiki_selected_id
            .clone()
            .or_else(|| self.wiki_article.as_ref().map(|a| a.id.clone()));
        self.cancel_wiki_edit();
        if let Some(id) = id {
            self.open_wiki_page_id_no_history(&id);
        }
    }

    /// Dispatch the save on the worker (EGUI_GRAPH_VIEW §2.5: no blocking IO on
    /// the UI thread). Edit buffers stay until `SavedPage` lands, so a 409/CAS
    /// conflict keeps the user's text; a repeated Save/Ctrl+S is ignored while
    /// a save is in flight.
    pub(super) fn save_wiki_edit(&mut self) {
        if self.pending_save.is_some() {
            return;
        }
        let Some(edit) = self.wiki_edit.as_ref() else {
            return;
        };
        let Some(art) = self.wiki_article.as_ref() else {
            self.wiki_error = Some("Нет открытой страницы для сохранения".into());
            return;
        };
        let Some(source) = self.available_load_source() else {
            self.wiki_error = Some("Для сохранения вики требуется HTTP gateway".into());
            return;
        };
        if !matches!(&source, LoadSource::Http(_)) {
            self.wiki_error = Some(
                "Режим --db доступен только для чтения; переподключитесь через HTTP gateway".into(),
            );
            return;
        }
        let req = WikiPutRequest {
            id: art.id.clone(),
            slug: slug_from_wiki_uri(&art.uri),
            uri: Some(art.uri.clone()),
            title: edit.title.clone(),
            content: edit.content.clone(),
            if_match_revision: edit.base_revision,
            if_match_etag: edit.base_etag.clone(),
        };
        let seq = self.next_seq();
        self.pending_save = Some(seq);
        self.worker.send(WorkerCmd::SavePage { seq, req, source });
    }

    /// Apply a worker SavedPage result (success refreshes catalog + backlinks;
    /// a conflict error keeps the edit buffers and offers Reload in the edit view).
    pub(super) fn apply_save_result(&mut self, result: Result<DocumentBody, String>) {
        match result {
            Ok(body) => {
                // Refresh catalog row for title/summary/revision.
                if let Some(meta) = self.wiki_pages.iter_mut().find(|p| p.id == body.id) {
                    meta.title = body.title.clone();
                    meta.revision = body.revision.unwrap_or(meta.revision);
                    meta.etag = body.etag.clone();
                    meta.updated_at = body.updated_at.clone();
                    if let Some(summary) = content_summary_line(&body.content) {
                        meta.summary = Some(summary);
                    }
                }
                sort_wiki_pages(&mut self.wiki_pages);
                self.refresh_backlinks(&body.id);
                let rev_note = body
                    .revision
                    .map(|r| format!(" · версия r{r}"))
                    .unwrap_or_default();
                self.wiki_save_note = Some(format!("Сохранено «{}»{rev_note}", body.title));
                self.wiki_article = Some(body);
                self.wiki_edit = None;
                self.wiki_error = None;
            }
            Err(e) => {
                self.wiki_error = Some(e);
                self.wiki_save_note = None;
            }
        }
    }

    pub(super) fn refresh_backlinks(&mut self, document_id: &str) {
        let Some(source) = self.available_load_source() else {
            return;
        };
        let seq = self.next_seq();
        self.pending_backlinks = Some(seq);
        self.wiki_backlinks_error = None;
        self.worker.send(WorkerCmd::LoadBacklinks {
            seq,
            document_id: document_id.to_string(),
            source,
            project: nonempty(&self.filter_wing),
        });
    }

    pub(super) fn wiki_known_keys(&self) -> (HashSet<String>, HashSet<String>) {
        let mut titles = HashSet::new();
        let mut slugs = HashSet::new();
        for p in &self.wiki_pages {
            titles.insert(p.title.clone());
            slugs.insert(p.slug.clone());
        }
        (titles, slugs)
    }

    pub(super) fn wiki_go_back(&mut self) {
        if self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед переходом назад.".into());
            return;
        }
        if self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("Есть несохранённые правки: сохраните или отмените их".into());
            return;
        }
        if let Some(prev) = self.wiki_history.pop() {
            self.wiki_focus = WikiPane::A;
            self.open_wiki_page_id_no_history(&prev);
        }
    }

    /// Resolve `[[link]]` text to a catalog page and open it (exact title/slug/id only).
    pub(super) fn open_wiki_link(&mut self, link: &str) {
        let into_b = self.wiki_dual_pane && self.wiki_focus == WikiPane::B;
        if !into_b && self.pending_save.is_some() {
            self.wiki_error =
                Some("Идёт сохранение. Дождитесь результата перед переходом по ссылке.".into());
            return;
        }
        if !into_b && self.wiki_edit.as_ref().is_some_and(|e| e.dirty) {
            self.wiki_error = Some("Есть несохранённые правки: сохраните или отмените их".into());
            return;
        }
        let q = link.trim();
        if q.is_empty() {
            return;
        }
        if let Some(p) = self.wiki_pages.iter().find(|p| {
            p.title == q
                || p.title.eq_ignore_ascii_case(q)
                || p.slug.eq_ignore_ascii_case(q)
                || p.id == q
                || p.uri == q
                || p.uri == format!("wiki://{q}")
        }) {
            let id = p.id.clone();
            self.open_wiki_page_id(&id);
            return;
        }
        if !self.filter_wing.trim().is_empty() {
            let msg = format!(
                "Ссылка [[{q}]] не найдена в проекте {}",
                self.filter_wing.trim()
            );
            if into_b {
                self.wiki_error_b = Some(msg);
            } else {
                self.wiki_error = Some(msg);
            }
            return;
        }
        // Exact wiki uri fallback (no fuzzy label pick) - fetched on the worker.
        let Some(source) = self.available_load_source() else {
            let msg = "Для wiki-ссылок требуется --http или --db".to_string();
            if into_b {
                self.wiki_error_b = Some(msg);
            } else {
                self.wiki_error = Some(msg);
            }
            return;
        };
        let seq = self.next_seq();
        if into_b {
            self.pending_page_b = Some(seq);
        } else {
            self.pending_page_a = Some(seq);
        }
        self.worker.send(WorkerCmd::OpenPage {
            seq,
            pane_b: into_b,
            push_history: !into_b,
            meta: None,
            q: Some(q.to_string()),
            source,
        });
    }

    pub(super) fn selected_graph_wiki_target(&self) -> Option<GraphWikiTarget> {
        let selected = self.selected.as_deref()?;
        let node = self
            .ui_graph
            .as_ref()?
            .nodes
            .iter()
            .find(|node| node.id == selected)?;
        graph_wiki_target(
            node.document_id.as_deref(),
            node.uri.as_deref(),
            &self.wiki_pages,
        )
    }

    pub(super) fn open_selected_graph_node_in_wiki(&mut self) {
        if self.available_load_source().is_none() {
            self.content_error = Some(
                "В snapshot-режиме доступна только топология; откройте HTTP или DB источник".into(),
            );
            return;
        }
        let Some(sel) = self.selected.clone() else {
            return;
        };
        let (doc_id, target) = {
            let Some(g) = self.ui_graph.as_ref() else {
                return;
            };
            let Some(node) = g.nodes.iter().find(|n| n.id == sel) else {
                return;
            };
            let target = graph_wiki_target(
                node.document_id.as_deref(),
                node.uri.as_deref(),
                &self.wiki_pages,
            );
            (node.document_id.clone(), target)
        };
        let Some(target) = target else {
            self.content_error = Some(
                "Выбранный узел не является wiki-страницей; используйте превью или корпус".into(),
            );
            return;
        };
        self.mode = ViewMode::Wiki;
        self.wiki_focus = WikiPane::A;
        match target {
            GraphWikiTarget::CatalogId(id) => self.open_wiki_page_id(&id),
            GraphWikiTarget::WikiUri(uri) => {
                if !self.filter_wing.trim().is_empty() {
                    if !self.wiki_loaded {
                        if let Some(id) = doc_id {
                            self.wiki_open_after_catalog = Some(id);
                            self.ensure_wiki_loaded();
                        } else {
                            self.wiki_error = Some(format!(
                                "Нельзя открыть {uri} в проекте {}, пока страница не появится в его wiki-каталоге",
                                self.filter_wing.trim()
                            ));
                        }
                    } else {
                        self.wiki_error = Some(format!(
                            "Wiki-страницы {uri} нет в проекте {}",
                            self.filter_wing.trim()
                        ));
                    }
                } else {
                    self.open_wiki_link(&uri);
                }
            }
        }
    }
}
